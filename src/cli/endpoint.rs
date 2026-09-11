use std::{
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use directories::BaseDirs;
use serde::{Deserialize, Serialize};

/// Directorio de datos de la aplicación dentro de `%LOCALAPPDATA%`.
const APPLICATION_DIRECTORY: &str = "GitHelper";
/// Archivo que publica el puerto y el token de la instancia en ejecución.
const ENDPOINT_FILE_NAME: &str = "instance-endpoint.json";
/// Versión del protocolo de instancia única.
pub const PROTOCOL_VERSION: u8 = 1;

/// Datos publicados por la instancia principal para que otras la contacten.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct InstanceEndpoint {
    /// Versión del protocolo soportada por el servidor.
    pub version: u8,
    /// Puerto efímero en el que escucha la instancia principal.
    pub port: u16,
    /// Token compartido que autentica al cliente.
    pub token: String,
}

impl InstanceEndpoint {
    /// Crea un endpoint con un token aleatorio.
    #[must_use]
    pub fn new(port: u16) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            port,
            token: uuid::Uuid::new_v4().simple().to_string(),
        }
    }
}

/// Ruta del archivo de endpoint para el usuario actual.
///
/// En Windows `%LOCALAPPDATA%` ya es privado por usuario, de modo que el token
/// no es legible desde otra sesión iniciada en la misma máquina.
pub fn endpoint_path() -> io::Result<PathBuf> {
    if let Some(override_path) = std::env::var_os("GIT_HELPER_INSTANCE_ENDPOINT") {
        return Ok(PathBuf::from(override_path));
    }
    let local_data = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| BaseDirs::new().map(|directories| directories.data_local_dir().to_owned()))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "no se pudo resolver el directorio de datos locales",
            )
        })?;
    Ok(local_data
        .join(APPLICATION_DIRECTORY)
        .join(ENDPOINT_FILE_NAME))
}

/// Publica el endpoint en disco con permisos restringidos al usuario.
pub fn write_endpoint(path: &Path, endpoint: &InstanceEndpoint) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let contents = serde_json::to_vec(endpoint)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut file = create_private_file(path)?;
    file.write_all(&contents)?;
    file.sync_all()?;
    Ok(())
}

/// Lee el endpoint publicado, si existe y es válido.
pub fn read_endpoint(path: &Path) -> io::Result<InstanceEndpoint> {
    let mut contents = Vec::new();
    File::open(path)?.read_to_end(&mut contents)?;
    let endpoint: InstanceEndpoint = serde_json::from_slice(&contents)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if endpoint.version != PROTOCOL_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("versión de protocolo no soportada: {}", endpoint.version),
        ));
    }
    if endpoint.port == 0 || endpoint.token.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "endpoint de instancia incompleto",
        ));
    }
    Ok(endpoint)
}

/// Elimina el endpoint solo si sigue apuntando a esta instancia.
///
/// Evita que una instancia secundaria borre el endpoint de la que está
/// realmente escuchando.
pub fn remove_endpoint_if_owned(path: &Path, owned: &InstanceEndpoint) {
    if read_endpoint(path).is_ok_and(|published| &published == owned) {
        let _ = std::fs::remove_file(path);
    }
}

/// Crea el archivo del endpoint legible solo por el usuario actual.
///
/// Los permisos se fijan en la propia creación para que el token nunca llegue a
/// existir en disco con un modo más laxo; `set_permissions` cubre además el caso
/// de un archivo que ya existiera con permisos heredados.
#[cfg(unix)]
fn create_private_file(path: &Path) -> io::Result<File> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

/// En Windows el archivo hereda las ACL de `%LOCALAPPDATA%`, que ya es privado
/// por usuario, así que basta con crearlo.
#[cfg(not(unix))]
fn create_private_file(path: &Path) -> io::Result<File> {
    File::create(path)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{InstanceEndpoint, read_endpoint, remove_endpoint_if_owned, write_endpoint};

    #[test]
    fn roundtrips_endpoint_file() {
        let directory = tempdir().expect("debe crear el directorio temporal");
        let path = directory.path().join("nested").join("endpoint.json");
        let endpoint = InstanceEndpoint::new(1234);

        write_endpoint(&path, &endpoint).expect("debe escribir el endpoint");
        assert_eq!(read_endpoint(&path).expect("debe leer"), endpoint);

        remove_endpoint_if_owned(&path, &endpoint);
        assert!(read_endpoint(&path).is_err());
    }

    #[test]
    fn keeps_endpoints_published_by_another_instance() {
        let directory = tempdir().expect("debe crear el directorio temporal");
        let path = directory.path().join("endpoint.json");
        let published = InstanceEndpoint::new(1234);
        write_endpoint(&path, &published).expect("debe escribir el endpoint");

        remove_endpoint_if_owned(&path, &InstanceEndpoint::new(4321));

        assert_eq!(read_endpoint(&path).expect("debe seguir ahí"), published);
    }

    #[test]
    fn rejects_incomplete_endpoints() {
        let directory = tempdir().expect("debe crear el directorio temporal");
        let path = directory.path().join("endpoint.json");
        std::fs::write(&path, br#"{"version":1,"port":0,"token":""}"#)
            .expect("debe escribir el archivo");

        assert!(read_endpoint(&path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn publishes_the_token_only_readable_by_the_owner() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().expect("debe crear el directorio temporal");
        let path = directory.path().join("endpoint.json");
        std::fs::write(&path, b"previo").expect("debe crear el archivo previo");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
            .expect("debe aflojar los permisos previos");

        write_endpoint(&path, &InstanceEndpoint::new(1234)).expect("debe escribir el endpoint");

        let mode = std::fs::metadata(&path)
            .expect("debe leer los metadatos")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn generates_distinct_tokens() {
        assert_ne!(
            InstanceEndpoint::new(1).token,
            InstanceEndpoint::new(1).token
        );
    }
}
