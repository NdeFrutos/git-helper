use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use async_channel::{Receiver, Sender};

/// Puerto local reservado para reenviar rutas a la instancia principal.
pub const INSTANCE_PORT: u16 = 39_271;

/// Mensaje recibido por la instancia principal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstanceRequest {
    /// Abre o activa un repositorio ya resuelto.
    OpenRepository(PathBuf),
    /// Solo activa la ventana existente.
    Activate,
}

/// Servidor TCP que acepta solicitudes de otras invocaciones de `ghelper`.
pub struct InstanceServer {
    stop: Arc<AtomicBool>,
}

impl InstanceServer {
    /// Arranca el servidor en segundo plano si el puerto está libre.
    pub fn start(request_sender: Sender<InstanceRequest>) -> io::Result<Option<Self>> {
        let address = instance_socket_address();
        let listener = match TcpListener::bind(address) {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => return Ok(None),
            Err(error) => return Err(error),
        };

        let stop = Arc::new(AtomicBool::new(false));
        let stop_flag = Arc::clone(&stop);
        thread::spawn(move || {
            for connection in listener.incoming() {
                if stop_flag.load(Ordering::Acquire) {
                    break;
                }
                let Ok(mut stream) = connection else {
                    continue;
                };
                let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                if let Some(request) = read_request(&mut stream) {
                    let _ = request_sender.send_blocking(request);
                }
            }
        });

        Ok(Some(Self { stop }))
    }

    /// Detiene el bucle de aceptación.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        let _ = TcpStream::connect(instance_socket_address());
    }
}

impl Drop for InstanceServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Intenta reenviar una solicitud a la instancia principal en ejecución.
pub fn forward_to_running_instance(request: &InstanceRequest) -> io::Result<()> {
    let mut stream = TcpStream::connect(instance_socket_address())?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    write_request(&mut stream, request)?;
    Ok(())
}

/// Indica si la instancia principal está escuchando solicitudes.
#[must_use]
pub fn is_instance_running() -> bool {
    TcpStream::connect(instance_socket_address()).is_ok()
}

fn instance_socket_address() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], INSTANCE_PORT))
}

fn write_request(stream: &mut TcpStream, request: &InstanceRequest) -> io::Result<()> {
    let payload = match request {
        InstanceRequest::Activate => String::new(),
        InstanceRequest::OpenRepository(path) => path.to_string_lossy().into_owned(),
    };
    let bytes = payload.as_bytes();
    let length = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "ruta demasiado larga"))?
        .to_le_bytes();
    stream.write_all(&length)?;
    stream.write_all(bytes)?;
    Ok(())
}

fn read_request(stream: &mut TcpStream) -> Option<InstanceRequest> {
    let mut length_bytes = [0_u8; 4];
    stream.read_exact(&mut length_bytes).ok()?;
    let length = u32::from_le_bytes(length_bytes) as usize;
    let mut payload = vec![0_u8; length];
    if length > 0 {
        stream.read_exact(&mut payload).ok()?;
    }
    let payload = String::from_utf8(payload).ok()?;
    if payload.is_empty() {
        return Some(InstanceRequest::Activate);
    }
    Some(InstanceRequest::OpenRepository(PathBuf::from(payload)))
}

/// Canal asíncrono para entregar solicitudes a la UI.
pub type InstanceRequestReceiver = Receiver<InstanceRequest>;

#[cfg(test)]
mod tests {
    use std::{
        net::{SocketAddr, TcpListener},
        thread,
    };

    use super::{InstanceRequest, read_request, write_request};

    #[test]
    fn serializes_activate_and_open_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("debe enlazar");
        let address = listener.local_addr().expect("debe obtener la dirección");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("debe aceptar");
            read_request(&mut stream)
        });

        let mut client = std::net::TcpStream::connect(address).expect("debe conectar");
        write_request(
            &mut client,
            &InstanceRequest::OpenRepository(std::path::PathBuf::from(r"C:\repos\demo")),
        )
        .expect("debe escribir");

        assert_eq!(
            server.join().expect("debe finalizar"),
            Some(InstanceRequest::OpenRepository(std::path::PathBuf::from(
                r"C:\repos\demo"
            )))
        );
    }

    #[test]
    fn roundtrips_activate_request() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("debe enlazar");
        let address: SocketAddr = listener.local_addr().expect("debe obtener la dirección");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("debe aceptar");
            read_request(&mut stream)
        });

        let mut client = std::net::TcpStream::connect(address).expect("debe conectar");
        write_request(&mut client, &InstanceRequest::Activate).expect("debe escribir");

        assert_eq!(
            server.join().expect("debe finalizar"),
            Some(InstanceRequest::Activate)
        );
    }
}
