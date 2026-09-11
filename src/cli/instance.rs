use std::{
    io::{self, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use async_channel::{Receiver, Sender};
use tracing::warn;

use super::{
    endpoint::{
        InstanceEndpoint, PROTOCOL_VERSION, endpoint_path, read_endpoint, remove_endpoint_if_owned,
        write_endpoint,
    },
    path::resolve_repository_path,
};
use crate::git::GitClient;

/// Marca que identifica el protocolo de instancia única.
const MAGIC: [u8; 4] = *b"GHLP";
/// Respuesta del servidor cuando acepta la solicitud.
const ACK_ACCEPTED: u8 = b'K';
/// Respuesta del servidor cuando rechaza la solicitud.
const ACK_REJECTED: u8 = b'X';
/// Tamaño máximo del payload; sobra para una ruta larga de Windows.
const MAX_PAYLOAD_BYTES: usize = 4 * 1024;
/// Tamaño máximo del token aceptado en el handshake.
const MAX_TOKEN_BYTES: usize = 64;
/// Tiempo máximo de espera en las operaciones de red del canal.
const IO_TIMEOUT: Duration = Duration::from_secs(2);

/// Mensaje recibido por la instancia principal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstanceRequest {
    /// Abre o activa un repositorio ya resuelto.
    OpenRepository(PathBuf),
    /// Solo activa la ventana existente.
    Activate,
}

/// Servidor local que acepta solicitudes autenticadas de otras invocaciones.
pub struct InstanceServer {
    stop: Arc<AtomicBool>,
    endpoint: InstanceEndpoint,
    endpoint_path: PathBuf,
}

impl InstanceServer {
    /// Arranca el servidor en un puerto efímero y publica su endpoint.
    ///
    /// El puerto es dinámico para evitar que otro programa ocupe uno fijo y para
    /// que varias sesiones de Windows puedan tener cada una su propia instancia.
    pub fn start(request_sender: Sender<InstanceRequest>) -> io::Result<Self> {
        let listener = TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))?;
        let port = listener.local_addr()?.port();
        let endpoint = InstanceEndpoint::new(port);
        let endpoint_path = endpoint_path()?;
        let expected_token = endpoint.token.clone();
        write_endpoint(&endpoint_path, &endpoint)?;

        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        thread::spawn(move || {
            let git_client = GitClient::default();
            for connection in listener.incoming() {
                if stop_flag.load(Ordering::Acquire) {
                    break;
                }
                let Ok(mut stream) = connection else {
                    continue;
                };
                let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
                let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
                let Some(request) = accepted_request(&mut stream, &expected_token, &git_client)
                else {
                    let _ = stream.write_all(&[ACK_REJECTED]);
                    continue;
                };
                if stream.write_all(&[ACK_ACCEPTED]).is_ok() {
                    let _ = request_sender.send_blocking(request);
                }
            }
        });

        Ok(Self {
            stop,
            endpoint,
            endpoint_path,
        })
    }

    /// Detiene el bucle de aceptación y retira el endpoint publicado.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        remove_endpoint_if_owned(&self.endpoint_path, &self.endpoint);
        let _ = TcpStream::connect(SocketAddr::from((Ipv4Addr::LOCALHOST, self.endpoint.port)));
    }
}

impl Drop for InstanceServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Errores al contactar con la instancia principal.
#[derive(Debug, thiserror::Error)]
pub enum ForwardError {
    #[error("no hay ninguna instancia de Git Helper publicada: {0}")]
    NoInstance(io::Error),
    #[error("no se pudo contactar con la instancia de Git Helper: {0}")]
    Transport(io::Error),
    #[error("la instancia que escucha no confirmó la solicitud")]
    NotAcknowledged,
}

/// Reenvía una solicitud autenticada a la instancia principal en ejecución.
///
/// Solo devuelve `Ok` si el interlocutor confirma con el ACK del protocolo, de
/// modo que un proceso ajeno escuchando en el puerto no puede hacer que la
/// aplicación termine sin abrir ventana.
pub fn forward_to_running_instance(request: &InstanceRequest) -> Result<(), ForwardError> {
    let path = endpoint_path().map_err(ForwardError::NoInstance)?;
    let endpoint = read_endpoint(&path).map_err(ForwardError::NoInstance)?;
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port));
    let mut stream =
        TcpStream::connect_timeout(&address, IO_TIMEOUT).map_err(ForwardError::Transport)?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(ForwardError::Transport)?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(ForwardError::Transport)?;
    write_request(&mut stream, &endpoint.token, request).map_err(ForwardError::Transport)?;

    let mut acknowledgement = [0_u8; 1];
    stream
        .read_exact(&mut acknowledgement)
        .map_err(|_| ForwardError::NotAcknowledged)?;
    if acknowledgement[0] == ACK_ACCEPTED {
        Ok(())
    } else {
        Err(ForwardError::NotAcknowledged)
    }
}

fn write_request(stream: &mut TcpStream, token: &str, request: &InstanceRequest) -> io::Result<()> {
    let payload = match request {
        InstanceRequest::Activate => String::new(),
        InstanceRequest::OpenRepository(path) => path.to_string_lossy().into_owned(),
    };
    let payload = payload.as_bytes();
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "ruta demasiado larga",
        ));
    }
    let token = token.as_bytes();
    let token_length = u8::try_from(token.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "token demasiado largo"))?;
    if usize::from(token_length) > MAX_TOKEN_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "token demasiado largo",
        ));
    }

    let mut frame = Vec::with_capacity(MAGIC.len() + 2 + token.len() + 4 + payload.len());
    frame.extend_from_slice(&MAGIC);
    frame.push(PROTOCOL_VERSION);
    frame.push(token_length);
    frame.extend_from_slice(token);
    let payload_length = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "ruta demasiado larga"))?;
    frame.extend_from_slice(&payload_length.to_le_bytes());
    frame.extend_from_slice(payload);
    stream.write_all(&frame)
}

fn read_request(stream: &mut TcpStream, expected_token: &str) -> io::Result<InstanceRequest> {
    let mut header = [0_u8; 6];
    stream.read_exact(&mut header)?;
    if header[..MAGIC.len()] != MAGIC {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "marca de protocolo desconocida",
        ));
    }
    if header[4] != PROTOCOL_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "versión de protocolo no soportada",
        ));
    }

    let token_length = usize::from(header[5]);
    if token_length == 0 || token_length > MAX_TOKEN_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "longitud de token inválida",
        ));
    }
    let mut token = vec![0_u8; token_length];
    stream.read_exact(&mut token)?;
    if !tokens_match(&token, expected_token.as_bytes()) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "token de instancia inválido",
        ));
    }

    let mut length_bytes = [0_u8; 4];
    stream.read_exact(&mut length_bytes)?;
    let length = u32::from_le_bytes(length_bytes) as usize;
    if length > MAX_PAYLOAD_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "payload demasiado grande",
        ));
    }
    let mut payload = vec![0_u8; length];
    if length > 0 {
        stream.read_exact(&mut payload)?;
    }
    let payload = String::from_utf8(payload)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "payload no es UTF-8"))?;
    if payload.is_empty() {
        return Ok(InstanceRequest::Activate);
    }
    Ok(InstanceRequest::OpenRepository(PathBuf::from(payload)))
}

/// Lee, autentica y revalida una solicitud entrante.
fn accepted_request(
    stream: &mut TcpStream,
    expected_token: &str,
    git_client: &GitClient,
) -> Option<InstanceRequest> {
    match read_request(stream, expected_token) {
        Ok(request) => validated_request(request, git_client),
        Err(error) => {
            warn!(error = %error, "Solicitud de instancia rechazada");
            None
        }
    }
}

/// Revalida la ruta recibida en el lado receptor antes de abrirla.
fn validated_request(request: InstanceRequest, git_client: &GitClient) -> Option<InstanceRequest> {
    match request {
        InstanceRequest::Activate => Some(InstanceRequest::Activate),
        InstanceRequest::OpenRepository(path) => match resolve_repository_path(&path, git_client) {
            Ok(root_path) => Some(InstanceRequest::OpenRepository(root_path)),
            Err(error) => {
                warn!(error = %error, "Ruta recibida por el canal de instancia descartada");
                None
            }
        },
    }
}

/// Compara tokens en tiempo constante respecto al contenido.
fn tokens_match(received: &[u8], expected: &[u8]) -> bool {
    if received.len() != expected.len() {
        return false;
    }
    received
        .iter()
        .zip(expected)
        .fold(0_u8, |accumulator, (left, right)| {
            accumulator | (left ^ right)
        })
        == 0
}

/// Canal asíncrono para entregar solicitudes a la UI.
pub type InstanceRequestReceiver = Receiver<InstanceRequest>;

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
        thread,
    };

    use super::{
        ACK_ACCEPTED, InstanceRequest, MAGIC, MAX_PAYLOAD_BYTES, read_request, tokens_match,
        write_request,
    };

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    type ServedRequest = Result<InstanceRequest, String>;

    fn serve_once(expected_token: &'static str) -> (SocketAddr, thread::JoinHandle<ServedRequest>) {
        let listener =
            TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).expect("debe enlazar");
        let address = listener.local_addr().expect("debe obtener la dirección");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("debe aceptar");
            match read_request(&mut stream, expected_token) {
                Ok(request) => {
                    let _ = stream.write_all(&[ACK_ACCEPTED]);
                    Ok(request)
                }
                Err(error) => Err(error.to_string()),
            }
        });
        (address, handle)
    }

    #[test]
    fn serializes_open_repository_request() {
        let (address, server) = serve_once(TOKEN);
        let mut client = TcpStream::connect(address).expect("debe conectar");
        write_request(
            &mut client,
            TOKEN,
            &InstanceRequest::OpenRepository(std::path::PathBuf::from(r"C:\repos\demo")),
        )
        .expect("debe escribir");
        let mut acknowledgement = [0_u8; 1];
        client
            .read_exact(&mut acknowledgement)
            .expect("debe recibir el ACK");

        assert_eq!(acknowledgement[0], ACK_ACCEPTED);
        assert_eq!(
            server.join().expect("debe finalizar"),
            Ok(InstanceRequest::OpenRepository(std::path::PathBuf::from(
                r"C:\repos\demo"
            )))
        );
    }

    #[test]
    fn roundtrips_activate_request() {
        let (address, server) = serve_once(TOKEN);
        let mut client = TcpStream::connect(address).expect("debe conectar");
        write_request(&mut client, TOKEN, &InstanceRequest::Activate).expect("debe escribir");

        assert_eq!(
            server.join().expect("debe finalizar"),
            Ok(InstanceRequest::Activate)
        );
    }

    #[test]
    fn rejects_requests_without_the_shared_token() {
        let (address, server) = serve_once(TOKEN);
        let mut client = TcpStream::connect(address).expect("debe conectar");
        write_request(&mut client, "token-invalido", &InstanceRequest::Activate)
            .expect("debe escribir");

        assert!(
            server
                .join()
                .expect("debe finalizar")
                .expect_err("debe rechazar")
                .contains("token de instancia inválido")
        );
    }

    #[test]
    fn rejects_unknown_protocol_magic() {
        let (address, server) = serve_once(TOKEN);
        let mut client = TcpStream::connect(address).expect("debe conectar");
        client.write_all(b"XXXX\x01\x20").expect("debe escribir");

        assert!(
            server
                .join()
                .expect("debe finalizar")
                .expect_err("debe rechazar")
                .contains("marca de protocolo desconocida")
        );
    }

    #[test]
    fn rejects_oversized_payload_lengths_without_allocating() {
        let (address, server) = serve_once(TOKEN);
        let mut client = TcpStream::connect(address).expect("debe conectar");
        let mut frame = Vec::new();
        frame.extend_from_slice(&MAGIC);
        frame.push(super::PROTOCOL_VERSION);
        frame.push(u8::try_from(TOKEN.len()).expect("token corto"));
        frame.extend_from_slice(TOKEN.as_bytes());
        frame.extend_from_slice(&u32::MAX.to_le_bytes());
        client.write_all(&frame).expect("debe escribir");

        assert!(
            server
                .join()
                .expect("debe finalizar")
                .expect_err("debe rechazar")
                .contains("payload demasiado grande")
        );
    }

    #[test]
    fn rejects_paths_longer_than_the_protocol_limit() {
        let (address, _server) = serve_once(TOKEN);
        let mut client = TcpStream::connect(address).expect("debe conectar");
        let long_path = "a".repeat(MAX_PAYLOAD_BYTES + 1);
        let error = write_request(
            &mut client,
            TOKEN,
            &InstanceRequest::OpenRepository(std::path::PathBuf::from(long_path)),
        )
        .expect_err("debe rechazar la ruta");

        assert!(error.to_string().contains("demasiado larga"));
    }

    #[test]
    fn compares_tokens_by_content() {
        assert!(tokens_match(b"abc", b"abc"));
        assert!(!tokens_match(b"abc", b"abd"));
        assert!(!tokens_match(b"abc", b"abcd"));
    }
}
