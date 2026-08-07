//! Sistema de ficheros con las garantías que exige el proyecto.
//!
//! La regla es "nunca dejar archivos corruptos". Se cumple con un patrón único
//! en todo el proyecto: **escribir en temporal → `fsync` → rename atómico**. Un
//! fichero presente en la biblioteca es, por definición, completo.

use std::path::Path;

use async_trait::async_trait;
use localify_core::error::{CoreError, CoreResult};
use localify_core::ports::platform::FileSystem;

#[derive(Debug, Clone, Copy, Default)]
pub struct RealFileSystem;

impl RealFileSystem {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

#[async_trait]
impl FileSystem for RealFileSystem {
    async fn atomic_rename(&self, from: &Path, to: &Path) -> CoreResult<()> {
        if let Some(padre) = to.parent() {
            tokio::fs::create_dir_all(padre).await.map_err(|e| {
                CoreError::storage(format!("no se pudo crear '{}': {e}", padre.display()))
            })?;
        }

        // `tokio::fs::rename` usa `MoveFileExW` con `MOVEFILE_REPLACE_EXISTING`
        // en Windows: es atómico dentro del mismo volumen y sobrescribe el
        // destino. Funciona con el fichero abierto por el motor de audio porque
        // este lo abre con FILE_SHARE_DELETE (ver `source/growing.rs`).
        match tokio::fs::rename(from, to).await {
            Ok(()) => Ok(()),
            Err(e) if e.raw_os_error() == Some(17) || es_cross_device(&e) => {
                // Volúmenes distintos: no hay rename atómico posible. Se copia
                // y se borra el origen. Sigue sin haber fichero incompleto
                // visible porque la copia va a un `.tmp` del destino.
                let temporal = to.with_extension("crossdev.tmp");
                tokio::fs::copy(from, &temporal).await.map_err(|e| {
                    CoreError::storage(format!("copia entre volúmenes fallida: {e}"))
                })?;
                tokio::fs::rename(&temporal, to)
                    .await
                    .map_err(|e| CoreError::storage(format!("rename tras copia fallido: {e}")))?;
                let _ = tokio::fs::remove_file(from).await;
                Ok(())
            }
            Err(e) => Err(CoreError::storage(format!(
                "no se pudo mover '{}' a '{}': {e}",
                from.display(),
                to.display()
            ))),
        }
    }

    async fn copy_file(&self, from: &Path, to: &Path) -> CoreResult<u64> {
        if let Some(padre) = to.parent() {
            tokio::fs::create_dir_all(padre).await.map_err(|e| {
                CoreError::storage(format!("no se pudo crear '{}': {e}", padre.display()))
            })?;
        }

        // Mismo patrón que el resto: temporal y rename. Sin él, una copia
        // interrumpida deja en el destino un fichero truncado con el nombre
        // definitivo, que es exactamente lo que el proyecto promete que no
        // ocurre.
        let temporal = to.with_extension("copy.tmp");
        let bytes = tokio::fs::copy(from, &temporal).await.map_err(|e| {
            CoreError::storage(format!(
                "no se pudo copiar '{}' a '{}': {e}",
                from.display(),
                to.display()
            ))
        })?;

        if let Err(e) = tokio::fs::rename(&temporal, to).await {
            let _ = tokio::fs::remove_file(&temporal).await;
            return Err(CoreError::storage(format!(
                "rename tras copiar a '{}' fallido: {e}",
                to.display()
            )));
        }
        Ok(bytes)
    }

    async fn write_synced(&self, path: &Path, bytes: &[u8]) -> CoreResult<()> {
        use tokio::io::AsyncWriteExt;

        if let Some(padre) = path.parent() {
            tokio::fs::create_dir_all(padre).await.map_err(|e| {
                CoreError::storage(format!("no se pudo crear '{}': {e}", padre.display()))
            })?;
        }

        // Escribir sobre un temporal y renombrar: si el proceso muere a mitad,
        // el fichero original sigue intacto en lugar de quedar truncado.
        let temporal = path.with_extension("writing.tmp");
        let mut f = tokio::fs::File::create(&temporal)
            .await
            .map_err(|e| CoreError::storage(format!("no se pudo crear el temporal: {e}")))?;
        f.write_all(bytes)
            .await
            .map_err(|e| CoreError::storage(format!("escritura fallida: {e}")))?;
        // `sync_all` antes del rename: sin esto, el rename puede llegar al
        // disco antes que los datos y dejar un fichero de ceros tras un corte.
        f.sync_all()
            .await
            .map_err(|e| CoreError::storage(format!("fsync fallido: {e}")))?;
        drop(f);

        self.atomic_rename(&temporal, path).await
    }

    async fn ensure_dir(&self, path: &Path) -> CoreResult<()> {
        tokio::fs::create_dir_all(path)
            .await
            .map_err(|e| CoreError::storage(format!("no se pudo crear '{}': {e}", path.display())))
    }

    async fn remove_file(&self, path: &Path) -> CoreResult<()> {
        match tokio::fs::remove_file(path).await {
            Ok(()) => Ok(()),
            // Que ya no exista es el resultado deseado, no un fallo.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(CoreError::storage(format!(
                "no se pudo borrar '{}': {e}",
                path.display()
            ))),
        }
    }

    async fn clear_dir(&self, path: &Path) -> CoreResult<u32> {
        let mut borrados = 0_u32;
        let mut entradas = match tokio::fs::read_dir(path).await {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(e) => {
                return Err(CoreError::storage(format!(
                    "no se pudo leer '{}': {e}",
                    path.display()
                )));
            }
        };

        while let Ok(Some(entrada)) = entradas.next_entry().await {
            let p = entrada.path();
            let resultado = if p.is_dir() {
                tokio::fs::remove_dir_all(&p).await
            } else {
                tokio::fs::remove_file(&p).await
            };
            match resultado {
                Ok(()) => borrados += 1,
                // Un fichero bloqueado no debe abortar la purga entera: se
                //  registra y se sigue. La próxima limpieza lo recogerá.
                Err(e) => tracing::warn!(path = %p.display(), error = %e, "no se pudo purgar"),
            }
        }
        Ok(borrados)
    }

    async fn exists(&self, path: &Path) -> bool {
        tokio::fs::try_exists(path).await.unwrap_or(false)
    }

    async fn file_size(&self, path: &Path) -> CoreResult<u64> {
        tokio::fs::metadata(path)
            .await
            .map(|m| m.len())
            .map_err(|e| CoreError::storage(format!("no se pudo leer '{}': {e}", path.display())))
    }

    async fn available_space(&self, path: &Path) -> CoreResult<u64> {
        espacio_disponible(path).await
    }

    async fn is_writable(&self, path: &Path) -> bool {
        // La única comprobación fiable es intentarlo: los permisos efectivos en
        // Windows dependen de ACLs, herencia y del propio usuario.
        let sonda = path.join(".localify-write-test");
        match tokio::fs::write(&sonda, b"1").await {
            Ok(()) => {
                let _ = tokio::fs::remove_file(&sonda).await;
                true
            }
            Err(_) => false,
        }
    }
}

fn es_cross_device(e: &std::io::Error) -> bool {
    // ERROR_NOT_SAME_DEVICE en Windows; EXDEV en Unix.
    matches!(e.raw_os_error(), Some(17 | 18))
}

#[cfg(windows)]
#[allow(
    clippy::unused_async,
    reason = "la consulta a Win32 es síncrona, pero la firma debe coincidir con la del resto de plataformas"
)]
async fn espacio_disponible(path: &Path) -> CoreResult<u64> {
    use std::os::windows::ffi::OsStrExt;

    use windows::Win32::Foundation::GetLastError;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);

    let mut disponibles: u64 = 0;
    // SAFETY: `wide` es una cadena UTF-16 terminada en nulo y viva durante toda
    // la llamada; los punteros de salida apuntan a variables locales válidas.
    let resultado = unsafe {
        windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
            windows::core::PCWSTR(wide.as_ptr()),
            Some(&raw mut disponibles),
            None,
            None,
        )
    };

    if resultado.is_ok() {
        return Ok(disponibles);
    }

    // SAFETY: `GetLastError` no tiene precondiciones.
    let code = unsafe { GetLastError() };
    Err(CoreError::storage(format!(
        "no se pudo consultar el espacio libre de '{}' (código {code:?})",
        path.display()
    )))
}

#[cfg(not(windows))]
async fn espacio_disponible(_path: &Path) -> CoreResult<u64> {
    // Se implementará al portar (statvfs). Devolver el máximo hace que las
    // comprobaciones de espacio nunca bloqueen en plataformas sin soporte.
    Ok(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir_temporal(nombre: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("localify-test-{nombre}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("crear dir de test");
        d
    }

    #[tokio::test]
    async fn write_synced_deja_el_contenido_completo_y_sin_temporales() {
        let dir = dir_temporal("write-synced");
        let destino = dir.join("sub").join("fichero.json");
        let fs = RealFileSystem::new();

        fs.write_synced(&destino, b"contenido")
            .await
            .expect("escribe");

        assert_eq!(tokio::fs::read(&destino).await.expect("lee"), b"contenido");
        let sobrantes: Vec<_> = std::fs::read_dir(destino.parent().expect("padre"))
            .expect("lista")
            .filter_map(Result::ok)
            .filter(|e| e.path().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(sobrantes.is_empty(), "quedaron ficheros temporales");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn write_synced_sobrescribe_sin_dejar_restos() {
        let dir = dir_temporal("overwrite");
        let destino = dir.join("f.txt");
        let fs = RealFileSystem::new();

        fs.write_synced(&destino, b"viejo largo largo")
            .await
            .expect("primera");
        fs.write_synced(&destino, b"nuevo").await.expect("segunda");

        assert_eq!(tokio::fs::read(&destino).await.expect("lee"), b"nuevo");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn borrar_algo_inexistente_no_es_un_error() {
        let fs = RealFileSystem::new();
        let inexistente = std::env::temp_dir().join("localify-no-existe-jamas.bin");
        assert!(fs.remove_file(&inexistente).await.is_ok());
    }

    #[tokio::test]
    async fn clear_dir_vacia_pero_conserva_el_directorio() {
        let dir = dir_temporal("clear");
        std::fs::write(dir.join("a.part"), b"x").expect("a");
        std::fs::write(dir.join("b.part"), b"y").expect("b");
        std::fs::create_dir_all(dir.join("sub")).expect("sub");

        let fs = RealFileSystem::new();
        let borrados = fs.clear_dir(&dir).await.expect("purga");

        assert_eq!(borrados, 3);
        assert!(dir.exists(), "el directorio .tmp debe seguir existiendo");
        assert_eq!(std::fs::read_dir(&dir).expect("lista").count(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn clear_dir_sobre_un_directorio_inexistente_devuelve_cero() {
        let fs = RealFileSystem::new();
        let d = std::env::temp_dir().join("localify-no-existe-dir");
        assert_eq!(fs.clear_dir(&d).await.expect("no falla"), 0);
    }

    #[tokio::test]
    async fn is_writable_detecta_una_carpeta_valida() {
        let dir = dir_temporal("writable");
        let fs = RealFileSystem::new();
        assert!(fs.is_writable(&dir).await);
        // No debe dejar la sonda detrás.
        assert_eq!(std::fs::read_dir(&dir).expect("lista").count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
