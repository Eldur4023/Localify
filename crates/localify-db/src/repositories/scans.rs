//! Informes del reconciliador de biblioteca.
//!
//! Se guarda el histórico completo, no solo el último. Ocupa unas decenas de
//! bytes por escaneo y permite responder a "¿desde cuándo falta este fichero?",
//! que es justo lo que alguien pregunta cuando algo desaparece.

use async_trait::async_trait;
use localify_core::domain::library::ScanReport;
use localify_core::error::CoreResult;
use localify_core::ports::database::ScanReportRepository;
use rusqlite::params;

use crate::error::ToCore;
use crate::pool::Pool;

pub struct SqliteScanReportRepository {
    pool: Pool,
}

impl std::fmt::Debug for SqliteScanReportRepository {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SqliteScanReportRepository")
            .finish_non_exhaustive()
    }
}

impl SqliteScanReportRepository {
    #[must_use]
    pub const fn new(pool: Pool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ScanReportRepository for SqliteScanReportRepository {
    async fn save(&self, report: &ScanReport) -> CoreResult<()> {
        let r = report.clone();
        self.pool
            .escribir(move |tx| {
                tx.execute(
                    "INSERT INTO scan_reports
                         (files_scanned, recovered, missing, unreadable, duration_ms)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        i64::from(r.files_scanned),
                        i64::from(r.recovered),
                        i64::from(r.missing),
                        i64::from(r.unreadable),
                        i64::try_from(r.duration_ms).unwrap_or(i64::MAX),
                    ],
                )?;
                Ok(())
            })
            .await
            .to_core()
    }

    async fn last(&self) -> CoreResult<Option<ScanReport>> {
        self.pool
            .leer(|conn| {
                let fila = conn.query_row(
                    "SELECT files_scanned, recovered, missing, unreadable, duration_ms
                     FROM scan_reports
                     ORDER BY finished_at DESC, id DESC
                     LIMIT 1",
                    [],
                    |r| {
                        Ok(ScanReport {
                            files_scanned: u32::try_from(r.get::<_, i64>(0)?.max(0)).unwrap_or(0),
                            recovered: u32::try_from(r.get::<_, i64>(1)?.max(0)).unwrap_or(0),
                            missing: u32::try_from(r.get::<_, i64>(2)?.max(0)).unwrap_or(0),
                            unreadable: u32::try_from(r.get::<_, i64>(3)?.max(0)).unwrap_or(0),
                            duration_ms: u64::try_from(r.get::<_, i64>(4)?.max(0)).unwrap_or(0),
                        })
                    },
                );
                Ok(fila.ok())
            })
            .await
            .to_core()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::TempDbGuard;

    async fn ctx() -> (SqliteScanReportRepository, TempDbGuard) {
        let (pool, guard) = Pool::temporal().expect("abre");
        crate::migrations::ejecutar(&pool).await.expect("migra");
        (SqliteScanReportRepository::new(pool), guard)
    }

    fn informe(recuperados: u32) -> ScanReport {
        ScanReport {
            files_scanned: 1000,
            recovered: recuperados,
            missing: 3,
            unreadable: 1,
            duration_ms: 4200,
        }
    }

    #[tokio::test]
    async fn sin_escaneos_no_hay_informe() {
        let (repo, _g) = ctx().await;
        assert!(repo.last().await.expect("consulta").is_none());
    }

    #[tokio::test]
    async fn el_informe_se_guarda_y_se_recupera_entero() {
        let (repo, _g) = ctx().await;
        repo.save(&informe(7)).await.expect("guarda");

        let leido = repo.last().await.expect("consulta").expect("existe");
        assert_eq!(leido.files_scanned, 1000);
        assert_eq!(leido.recovered, 7);
        assert_eq!(leido.missing, 3);
        assert_eq!(leido.unreadable, 1);
        assert_eq!(leido.duration_ms, 4200);
    }

    #[tokio::test]
    async fn se_devuelve_el_mas_reciente() {
        // `finished_at` tiene resolucion de segundo: dos escaneos seguidos
        // comparten marca de tiempo, y sin desempatar por `id` se devolveria
        // uno cualquiera de los dos.
        let (repo, _g) = ctx().await;
        for n in [1_u32, 2, 3] {
            repo.save(&informe(n)).await.expect("guarda");
        }

        assert_eq!(
            repo.last()
                .await
                .expect("consulta")
                .expect("existe")
                .recovered,
            3,
            "deberia devolver el ultimo escaneo, no el primero"
        );
    }

    #[tokio::test]
    async fn se_conserva_el_historico() {
        // Permite responder a "desde cuando falta este fichero", que es lo que
        // alguien pregunta cuando algo desaparece.
        let (repo, _g) = ctx().await;
        for n in 0..5_u32 {
            repo.save(&informe(n)).await.expect("guarda");
        }
        assert!(repo.last().await.expect("consulta").is_some());
    }
}
