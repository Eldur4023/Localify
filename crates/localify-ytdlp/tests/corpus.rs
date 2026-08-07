//! Corpus de emparejamiento.
//!
//! Es el criterio de calidad de la Fase 6: un conjunto de casos reales con la
//! respuesta correcta anotada a mano. Cubre los escenarios que más daño hacen
//! si se resuelven mal, porque **lo descargado no se vuelve a descargar**: un
//! karaoke que entre en la biblioteca se queda para siempre.
//!
//! Cada caso declara los candidatos que devolvería YouTube y cuál es el bueno.
//! Sin red y sin yt-dlp: son datos.

#![allow(clippy::expect_used, clippy::panic)]

use localify_core::domain::audio::DurationMs;
use localify_core::domain::download::Confidence;
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::track::{AlbumRef, ArtistRef, Track};
use localify_ytdlp::scoring::elegir_mejor;
use localify_ytdlp::search::RawCandidate;

/// Construye la pista de referencia.
fn pista(titulo: &str, artista: &str, album: Option<&str>, segundos: u32) -> Track {
    Track {
        id: TrackId::nuevo_local(),
        title: titulo.to_owned(),
        album: album.map(|a| AlbumRef {
            id: AlbumId::nuevo_local(),
            title: a.to_owned(),
        }),
        artists: vec![ArtistRef {
            id: ArtistId::nuevo_local(),
            name: artista.to_owned(),
        }],
        duration: DurationMs::from_secs(segundos),
        track_number: None,
        disc_number: None,
        explicit: false,
        isrc: None,
        release_date: None,
        popularity: Some(70),
        added_at: chrono::Utc::now(),
    }
}

/// Constructor de candidatos con valores razonables por defecto.
struct Candidato {
    c: RawCandidate,
}

impl Candidato {
    fn nuevo(id: &str, titulo: &str, canal: &str, segundos: u32) -> Self {
        Self {
            c: RawCandidate {
                video_id: id.to_owned(),
                title: titulo.to_owned(),
                channel: Some(canal.to_owned()),
                description: None,
                duration: DurationMs::from_secs(segundos),
                view_count: Some(500_000),
                from_youtube_music: false,
                provided_to_youtube: false,
            },
        }
    }

    fn music(mut self) -> Self {
        self.c.from_youtube_music = true;
        self
    }

    fn oficial(mut self) -> Self {
        self.c.provided_to_youtube = true;
        self.c.description = Some("Provided to YouTube by Universal Music Group".to_owned());
        self
    }

    fn vistas(mut self, v: u64) -> Self {
        self.c.view_count = Some(v);
        self
    }

    fn build(self) -> RawCandidate {
        self.c
    }
}

/// Comprueba que gana el candidato esperado y con que confianza.
fn elige(pista: &Track, candidatos: &[RawCandidate], esperado: &str) -> Confidence {
    let resultado = elegir_mejor(pista, candidatos, &[]).expect("hay candidatos");

    assert_eq!(
        resultado.best.video_id, esperado,
        "\nEsperado: '{esperado}'\nElegido:  '{}' ({:.1} puntos)\nMotivos: {:?}",
        resultado.best.video_id, resultado.best.score, resultado.best.breakdown.penalty_reasons
    );

    resultado.confidence
}

// ============================================================================
// Casos que deben resolverse bien
// ============================================================================

#[test]
fn el_canal_topic_gana_al_videoclip_oficial() {
    let p = pista(
        "Bohemian Rhapsody",
        "Queen",
        Some("A Night at the Opera"),
        354,
    );
    let confianza = elige(
        &p,
        &[
            Candidato::nuevo("topic", "Bohemian Rhapsody", "Queen - Topic", 354)
                .oficial()
                .build(),
            Candidato::nuevo(
                "clip",
                "Queen - Bohemian Rhapsody (Official Video Remastered)",
                "Queen Official",
                355,
            )
            .vistas(1_800_000_000)
            .build(),
        ],
        "topic",
    );
    assert_eq!(confianza, Confidence::High);
}

#[test]
fn el_karaoke_pierde_aunque_dure_exactamente_lo_mismo() {
    // El caso mas peligroso: duracion identica y titulo casi igual.
    let p = pista("Under Pressure", "Queen", Some("Hot Space"), 248);
    elige(
        &p,
        &[
            Candidato::nuevo(
                "karaoke",
                "Under Pressure - Karaoke Version",
                "Sing King",
                248,
            )
            .vistas(3_000_000)
            .build(),
            Candidato::nuevo("bueno", "Under Pressure", "Queen - Topic", 248)
                .oficial()
                .build(),
        ],
        "bueno",
    );
}

#[test]
fn el_directo_pierde_frente_a_la_version_de_estudio() {
    let p = pista("Smells Like Teen Spirit", "Nirvana", Some("Nevermind"), 301);
    elige(
        &p,
        &[
            Candidato::nuevo(
                "live",
                "Smells Like Teen Spirit (Live at Reading 1992)",
                "Nirvana - Topic",
                305,
            )
            .oficial()
            .build(),
            Candidato::nuevo("estudio", "Smells Like Teen Spirit", "Nirvana - Topic", 301)
                .oficial()
                .build(),
        ],
        "estudio",
    );
}

#[test]
fn una_cancion_que_es_un_directo_encuentra_su_directo() {
    // La excepcion del sistema: si Spotify dice "Live", penalizar "live"
    // dejaria a esta cancion sin coincidencia posible.
    let p = pista(
        "Smells Like Teen Spirit - Live at Reading",
        "Nirvana",
        Some("Live at Reading"),
        305,
    );
    let confianza = elige(
        &p,
        &[
            Candidato::nuevo("estudio", "Smells Like Teen Spirit", "Nirvana - Topic", 301)
                .oficial()
                .build(),
            Candidato::nuevo(
                "live",
                "Smells Like Teen Spirit (Live at Reading)",
                "Nirvana - Topic",
                305,
            )
            .oficial()
            .build(),
        ],
        "live",
    );
    assert!(
        confianza.permite_descarga_automatica(),
        "un directo legitimo debe poder descargarse"
    );
}

#[test]
fn un_remix_legitimo_encuentra_su_remix() {
    let p = pista("Sandstorm - Extended Remix", "Darude", None, 220);
    elige(
        &p,
        &[
            Candidato::nuevo("original", "Sandstorm", "Darude - Topic", 225)
                .oficial()
                .build(),
            Candidato::nuevo("remix", "Sandstorm (Extended Remix)", "Darude - Topic", 220)
                .oficial()
                .build(),
        ],
        "remix",
    );
}

#[test]
fn el_audio_manipulado_pierde() {
    let p = pista("Teardrop", "Massive Attack", Some("Mezzanine"), 330);
    elige(
        &p,
        &[
            Candidato::nuevo("slowed", "Teardrop (slowed + reverb)", "Vibes Channel", 330)
                .vistas(8_000_000)
                .build(),
            Candidato::nuevo("bueno", "Teardrop", "Massive Attack - Topic", 330)
                .oficial()
                .build(),
        ],
        "bueno",
    );
}

#[test]
fn un_recopilatorio_de_una_hora_no_se_confunde_con_una_cancion() {
    let p = pista("Around the World", "Daft Punk", Some("Homework"), 428);
    elige(
        &p,
        &[
            Candidato::nuevo(
                "recopilatorio",
                "Daft Punk Greatest Hits Full Album",
                "Music Mix",
                3600,
            )
            .vistas(20_000_000)
            .build(),
            Candidato::nuevo("bueno", "Around the World", "Daft Punk - Topic", 428)
                .oficial()
                .build(),
        ],
        "bueno",
    );
}

#[test]
fn youtube_music_gana_a_un_canal_cualquiera_con_igualdad_de_condiciones() {
    let p = pista("Digital Love", "Daft Punk", Some("Discovery"), 301);
    elige(
        &p,
        &[
            Candidato::nuevo(
                "cualquiera",
                "Daft Punk - Digital Love",
                "MusicLover99",
                301,
            )
            .build(),
            Candidato::nuevo("music", "Digital Love", "Daft Punk", 301)
                .music()
                .build(),
        ],
        "music",
    );
}

#[test]
fn una_resubida_sin_recorrido_pierde_frente_al_canal_oficial() {
    let p = pista("Roads", "Portishead", Some("Dummy"), 302);
    elige(
        &p,
        &[
            Candidato::nuevo("resubida", "Portishead - Roads", "user8271", 302)
                .vistas(120)
                .build(),
            Candidato::nuevo("bueno", "Roads", "Portishead - Topic", 302)
                .oficial()
                .build(),
        ],
        "bueno",
    );
}

#[test]
fn los_diacriticos_no_impiden_la_coincidencia() {
    let p = pista("Jóga", "Björk", Some("Homogenic"), 305);
    let confianza = elige(
        &p,
        &[
            Candidato::nuevo("bueno", "Bjork - Joga", "Björk - Topic", 305)
                .oficial()
                .build(),
        ],
        "bueno",
    );
    assert!(confianza.permite_descarga_automatica());
}

#[test]
fn un_titulo_en_japones_se_empareja_igual() {
    let p = pista("君の名は", "RADWIMPS", None, 240);
    let confianza = elige(
        &p,
        &[
            Candidato::nuevo("bueno", "RADWIMPS - 君の名は", "RADWIMPS - Topic", 240)
                .oficial()
                .build(),
        ],
        "bueno",
    );
    assert!(
        confianza.permite_descarga_automatica(),
        "el sistema no debe asumir alfabeto latino"
    );
}

#[test]
fn una_cancion_con_live_dentro_de_otra_palabra_no_se_penaliza() {
    // "Stayin' Alive" contiene "live". Con busqueda por subcadena, la version
    // correcta se descartaria como si fuera una grabacion en directo.
    let p = pista(
        "Stayin Alive",
        "Bee Gees",
        Some("Saturday Night Fever"),
        285,
    );
    let confianza = elige(
        &p,
        &[
            Candidato::nuevo("bueno", "Stayin Alive", "Bee Gees - Topic", 285)
                .oficial()
                .build(),
        ],
        "bueno",
    );
    assert_eq!(
        confianza,
        Confidence::High,
        "la coincidencia debe ser por palabra completa, no por subcadena"
    );
}

// ============================================================================
// Casos donde lo correcto es NO descargar
// ============================================================================

#[test]
fn sin_ningun_candidato_decente_no_se_descarga() {
    // ADR-017: mejor una biblioteca mas pequena que una con basura.
    let p = pista("Una Cancion Muy Rara", "Artista Desconocido", None, 200);
    let resultado = elegir_mejor(
        &p,
        &[
            Candidato::nuevo("malo1", "Otra Cosa Completamente Distinta", "Random", 400)
                .vistas(50)
                .build(),
            Candidato::nuevo("malo2", "Karaoke Mix Vol 3", "Karaoke Channel", 3600).build(),
        ],
        &[],
    )
    .expect("hay candidatos");

    assert_eq!(resultado.confidence, Confidence::Low);
    assert!(
        !resultado.confidence.permite_descarga_automatica(),
        "score {:.1}: nada de esto deberia entrar en la biblioteca",
        resultado.best.score
    );
}

#[test]
fn una_duracion_disparatada_descarta_el_candidato() {
    let p = pista("Under Pressure", "Queen", Some("Hot Space"), 248);
    let resultado = elegir_mejor(
        &p,
        &[
            // Titulo y canal perfectos, pero dura cinco minutos mas.
            Candidato::nuevo("largo", "Under Pressure", "Queen - Topic", 550)
                .oficial()
                .build(),
        ],
        &[],
    )
    .expect("hay candidatos");

    assert_eq!(
        resultado.confidence,
        Confidence::Low,
        "ninguna bonificacion debe rescatar una duracion imposible"
    );
    assert!(
        resultado
            .best
            .breakdown
            .penalty_reasons
            .contains(&"duration.discarded".to_owned())
    );
}

#[test]
fn falta_lo_que_spotify_declara_y_se_penaliza() {
    // Spotify dice "Acoustic": un candidato electrico es la version equivocada.
    let p = pista("Layla - Acoustic", "Eric Clapton", None, 285);
    let resultado = elegir_mejor(
        &p,
        &[
            Candidato::nuevo("electrico", "Layla", "Eric Clapton - Topic", 285)
                .oficial()
                .build(),
        ],
        &[],
    )
    .expect("hay candidatos");

    assert!(
        resultado
            .best
            .breakdown
            .penalty_reasons
            .iter()
            .any(|m| m.starts_with("missing.")),
        "debe anotarse que falta el termino requerido: {:?}",
        resultado.best.breakdown.penalty_reasons
    );
}

// ============================================================================
// Propiedades del sistema
// ============================================================================

#[test]
fn un_candidato_excluido_no_vuelve_a_elegirse() {
    let p = pista("Under Pressure", "Queen", Some("Hot Space"), 248);
    let candidatos = [
        Candidato::nuevo("rechazado", "Under Pressure", "Queen - Topic", 248)
            .oficial()
            .build(),
        Candidato::nuevo(
            "alternativo",
            "Queen - Under Pressure",
            "Queen Official",
            249,
        )
        .build(),
    ];

    let resultado =
        elegir_mejor(&p, &candidatos, &["rechazado".to_owned()]).expect("queda una alternativa");

    assert_eq!(
        resultado.best.video_id, "alternativo",
        "si el usuario dijo que no era, volver a elegirlo seria ignorarle"
    );
}

#[test]
fn excluir_todos_los_candidatos_no_devuelve_ninguno() {
    let p = pista("X", "Y", None, 200);
    let candidatos = [Candidato::nuevo("solo", "X", "Y - Topic", 200).build()];

    assert!(elegir_mejor(&p, &candidatos, &["solo".to_owned()]).is_none());
}

#[test]
fn sin_candidatos_no_hay_resultado() {
    let p = pista("X", "Y", None, 200);
    assert!(elegir_mejor(&p, &[], &[]).is_none());
}

#[test]
fn la_eleccion_es_determinista_ante_empates() {
    // Un emparejamiento que cambia entre reintentos es imposible de depurar.
    let p = pista("Under Pressure", "Queen", Some("Hot Space"), 248);
    let candidatos = [
        Candidato::nuevo("zzz", "Under Pressure", "Queen - Topic", 248).build(),
        Candidato::nuevo("aaa", "Under Pressure", "Queen - Topic", 248).build(),
    ];

    for _ in 0..10 {
        let r = elegir_mejor(&p, &candidatos, &[]).expect("hay candidatos");
        assert_eq!(r.best.video_id, "aaa", "el desempate debe ser estable");
    }
}

#[test]
fn el_desglose_explica_la_puntuacion() {
    // Sin trazabilidad, depurar un mal emparejamiento es adivinar.
    let p = pista("Under Pressure", "Queen", Some("Hot Space"), 248);
    let r = elegir_mejor(
        &p,
        &[
            Candidato::nuevo("x", "Under Pressure", "Queen - Topic", 250)
                .oficial()
                .build(),
        ],
        &[],
    )
    .expect("hay candidatos");

    let d = &r.best.breakdown;
    assert!(d.source_bonus > 0.0, "el canal Topic debe puntuar");
    assert!(d.title_bonus > 0.0, "el titulo coincide");
    assert!(d.artist_bonus > 0.0, "el artista coincide");
    assert_eq!(d.duration_diff_ms, 2000);
    assert!((d.duration_factor - 1.0).abs() < f32::EPSILON);
    assert!((d.total - r.best.score).abs() < 0.01);
}

#[test]
fn la_puntuacion_nunca_se_sale_del_rango() {
    let p = pista("Under Pressure", "Queen", Some("Hot Space"), 248);
    let casos = [
        // Todo a favor.
        Candidato::nuevo("perfecto", "Under Pressure Hot Space", "Queen - Topic", 248)
            .music()
            .oficial()
            .build(),
        // Todo en contra.
        Candidato::nuevo(
            "pesimo",
            "Karaoke Live Cover Remix slowed reverb 8d Full Album Mix",
            "Random",
            3600,
        )
        .vistas(3)
        .build(),
    ];

    for c in casos {
        let id = c.video_id.clone();
        let r = elegir_mejor(&p, std::slice::from_ref(&c), &[]).expect("hay candidato");
        assert!(
            (0.0..=100.0).contains(&r.best.score),
            "'{id}' puntuo {} fuera de [0, 100]",
            r.best.score
        );
    }
}

#[test]
fn el_numero_de_candidatos_considerados_se_informa() {
    let p = pista("Under Pressure", "Queen", None, 248);
    let candidatos: Vec<RawCandidate> = (0..7)
        .map(|i| Candidato::nuevo(&format!("v{i}"), "Under Pressure", "Queen - Topic", 248).build())
        .collect();

    let r = elegir_mejor(&p, &candidatos, &[]).expect("hay candidatos");
    assert_eq!(r.candidates_considered, 7);
}
