//! Corpus amplio de emparejamiento: 50 canciones anotadas a mano.
//!
//! Es el criterio de calidad de la Fase 6. Los tests de `corpus.rs` comprueban
//! reglas concretas de una en una; este mide la **tasa de acierto agregada**
//! sobre un conjunto variado: pop, rock, clásica, electrónica, remixes
//! legítimos, directos legítimos, temas en japonés, coreano, alemán, francés y
//! español, canciones de 23 segundos y de 23 minutos, con y sin álbum.
//!
//! Dos umbrales, y el segundo es el que de verdad importa:
//!
//! 1. **≥ 90 % de aciertos.** Fallar al encontrar una canción es molesto.
//! 2. **Cero falsos positivos.** Elegir un karaoke, un cover o un "8D audio"
//!    es mucho peor: lo descargado no se vuelve a descargar (ADR-017), así que
//!    esa basura se queda en la biblioteca para siempre.
//!
//! Por eso los candidatos contaminados llevan el prefijo `basura-` en su
//! identificador: el test puede detectar automáticamente si alguno se cuela,
//! sin depender de que el caso concreto estuviera en la lista de fallos.
//!
//! Sin red y sin yt-dlp: son datos.

#![allow(clippy::expect_used, clippy::panic)]

use localify_core::domain::audio::DurationMs;
use localify_core::domain::ids::{AlbumId, ArtistId, TrackId};
use localify_core::domain::track::{AlbumRef, ArtistRef, Track};
use localify_ytdlp::scoring::elegir_mejor;
use localify_ytdlp::search::RawCandidate;

// ─────────────────────────────────────────────────────────────────────────────
// Construcción de casos
// ─────────────────────────────────────────────────────────────────────────────

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

/// Candidato de canal Topic con audio subido por la discográfica.
fn topic(id: &str, titulo: &str, artista: &str, segundos: u32) -> RawCandidate {
    RawCandidate {
        video_id: id.to_owned(),
        title: titulo.to_owned(),
        channel: Some(format!("{artista} - Topic")),
        description: Some("Provided to YouTube by Universal Music Group".to_owned()),
        duration: DurationMs::from_secs(segundos),
        view_count: Some(500_000),
        from_youtube_music: false,
        provided_to_youtube: true,
    }
}

/// Candidato de YouTube Music.
fn music(id: &str, titulo: &str, artista: &str, segundos: u32) -> RawCandidate {
    RawCandidate {
        video_id: id.to_owned(),
        title: titulo.to_owned(),
        channel: Some(artista.to_owned()),
        description: None,
        duration: DurationMs::from_secs(segundos),
        view_count: Some(500_000),
        from_youtube_music: true,
        provided_to_youtube: false,
    }
}

/// Candidato de un canal cualquiera, con las visitas que tenga.
fn suelto(id: &str, titulo: &str, canal: &str, segundos: u32, vistas: u64) -> RawCandidate {
    RawCandidate {
        video_id: id.to_owned(),
        title: titulo.to_owned(),
        channel: Some(canal.to_owned()),
        description: None,
        duration: DurationMs::from_secs(segundos),
        view_count: Some(vistas),
        from_youtube_music: false,
        provided_to_youtube: false,
    }
}

struct Caso {
    nombre: &'static str,
    pista: Track,
    candidatos: Vec<RawCandidate>,
    /// `None` cuando lo correcto es **no** descargar nada.
    esperado: Option<&'static str>,
}

fn caso(
    nombre: &'static str,
    pista: Track,
    candidatos: Vec<RawCandidate>,
    esperado: Option<&'static str>,
) -> Caso {
    Caso {
        nombre,
        pista,
        candidatos,
        esperado,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// El corpus
// ─────────────────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_lines, reason = "es una tabla de datos, no lógica")]
fn corpus() -> Vec<Caso> {
    vec![
        // ── Pop y rock: el caso común ───────────────────────────────────────
        caso(
            "billie-jean: el karaoke no gana aunque tenga millones de visitas",
            pista("Billie Jean", "Michael Jackson", Some("Thriller"), 294),
            vec![
                suelto(
                    "basura-karaoke",
                    "Billie Jean (Karaoke Version)",
                    "Sing2Music",
                    294,
                    2_000_000,
                ),
                topic("bueno", "Billie Jean", "Michael Jackson", 294),
            ],
            Some("bueno"),
        ),
        caso(
            "wonderwall: un cover acústico popular no sustituye al original",
            pista(
                "Wonderwall",
                "Oasis",
                Some("(What's the Story) Morning Glory?"),
                258,
            ),
            vec![
                suelto(
                    "basura-cover",
                    "Wonderwall (Acoustic Cover)",
                    "GuitarGuy",
                    260,
                    5_000_000,
                ),
                topic("bueno", "Wonderwall", "Oasis", 258),
            ],
            Some("bueno"),
        ),
        caso(
            "take-on-me: el 8D audio pierde frente a YouTube Music",
            pista("Take On Me", "a-ha", Some("Hunting High and Low"), 225),
            vec![
                suelto(
                    "basura-8d",
                    "Take On Me (8D Audio)",
                    "8D Tunes",
                    225,
                    10_000_000,
                ),
                music("bueno", "Take On Me", "a-ha", 225),
            ],
            Some("bueno"),
        ),
        caso(
            "hotel-california: la version de estudio gana al directo",
            pista("Hotel California", "Eagles", Some("Hotel California"), 391),
            vec![
                topic(
                    "live",
                    "Hotel California (Live at the Forum '76)",
                    "Eagles",
                    428,
                ),
                topic("bueno", "Hotel California", "Eagles", 391),
            ],
            Some("bueno"),
        ),
        caso(
            "zombie: karaoke con duracion identica",
            pista("Zombie", "The Cranberries", Some("No Need to Argue"), 307),
            vec![
                suelto(
                    "basura-karaoke",
                    "Zombie - Karaoke",
                    "KaraFun",
                    307,
                    1_500_000,
                ),
                topic("bueno", "Zombie", "The Cranberries", 307),
            ],
            Some("bueno"),
        ),
        caso(
            "africa: slowed + reverb",
            pista("Africa", "Toto", Some("Toto IV"), 295),
            vec![
                suelto(
                    "basura-slowed",
                    "Africa (slowed + reverb)",
                    "chill vibes",
                    320,
                    8_000_000,
                ),
                topic("bueno", "Africa", "Toto", 295),
            ],
            Some("bueno"),
        ),
        caso(
            "yesterday: cancion corta con cover al piano",
            pista("Yesterday", "The Beatles", Some("Help!"), 125),
            vec![
                suelto(
                    "basura-cover",
                    "Yesterday (Piano Cover)",
                    "PianoRoom",
                    130,
                    900_000,
                ),
                topic("bueno", "Yesterday", "The Beatles", 125),
            ],
            Some("bueno"),
        ),
        caso(
            "numb: bass boosted",
            pista("Numb", "Linkin Park", Some("Meteora"), 187),
            vec![
                suelto(
                    "basura-bass",
                    "Numb (Bass Boosted)",
                    "BassNation",
                    187,
                    4_000_000,
                ),
                topic("bueno", "Numb", "Linkin Park", 187),
            ],
            Some("bueno"),
        ),
        caso(
            "comfortably-numb: un concierto entero no es una cancion",
            pista("Comfortably Numb", "Pink Floyd", Some("The Wall"), 382),
            vec![
                suelto(
                    "basura-concierto",
                    "Pink Floyd Live in Pompeii Full Concert",
                    "Rock Archive",
                    5400,
                    12_000_000,
                ),
                topic("bueno", "Comfortably Numb", "Pink Floyd", 382),
            ],
            Some("bueno"),
        ),
        caso(
            "nothing-else-matters: version de un grupo tributo",
            pista("Nothing Else Matters", "Metallica", Some("Metallica"), 388),
            vec![
                suelto(
                    "basura-cover",
                    "Nothing Else Matters (Cover)",
                    "MetalCovers",
                    388,
                    700_000,
                ),
                topic("bueno", "Nothing Else Matters", "Metallica", 388),
            ],
            Some("bueno"),
        ),
        caso(
            "master-of-puppets: guitar cover",
            pista(
                "Master of Puppets",
                "Metallica",
                Some("Master of Puppets"),
                515,
            ),
            vec![
                suelto(
                    "basura-cover",
                    "Master of Puppets Guitar Cover",
                    "ShredDaily",
                    515,
                    3_000_000,
                ),
                topic("bueno", "Master of Puppets", "Metallica", 515),
            ],
            Some("bueno"),
        ),
        caso(
            "creep: gana la duracion mas cercana con todo lo demas igual",
            pista("Creep", "Radiohead", Some("Pablo Honey"), 238),
            vec![
                topic("casi", "Creep", "Radiohead", 244),
                topic("exacto", "Creep", "Radiohead", 238),
            ],
            Some("exacto"),
        ),
        // ── Clásica: títulos largos y recopilatorios de horas ────────────────
        caso(
            "clair-de-lune: tres horas de piano relajante no son la pieza",
            pista(
                "Clair de Lune",
                "Claude Debussy",
                Some("Suite bergamasque"),
                300,
            ),
            vec![
                suelto(
                    "basura-recopilatorio",
                    "3 Hours of Relaxing Piano Music",
                    "Calm Music",
                    10800,
                    30_000_000,
                ),
                topic("bueno", "Clair de Lune", "Claude Debussy", 300),
            ],
            Some("bueno"),
        ),
        caso(
            "fur-elise: greatest hits full album",
            pista("Für Elise", "Ludwig van Beethoven", None, 175),
            vec![
                suelto(
                    "basura-recopilatorio",
                    "Beethoven Greatest Hits Full Album",
                    "Classical Vault",
                    3600,
                    15_000_000,
                ),
                topic("bueno", "Fur Elise", "Ludwig van Beethoven", 175),
            ],
            Some("bueno"),
        ),
        caso(
            "vivaldi-primavera: titulo clasico muy largo",
            pista(
                "The Four Seasons, Violin Concerto in E Major, Op. 8 No. 1, RV 269 \"Spring\": I. Allegro",
                "Antonio Vivaldi",
                Some("The Four Seasons"),
                200,
            ),
            vec![
                topic(
                    "bueno",
                    "The Four Seasons, Violin Concerto in E Major, RV 269 \"Spring\": I. Allegro",
                    "Antonio Vivaldi",
                    202,
                ),
                suelto(
                    "otro",
                    "Vivaldi Four Seasons Complete",
                    "Classic FM",
                    2400,
                    900_000,
                ),
            ],
            Some("bueno"),
        ),
        caso(
            "beethoven-5: movimiento suelto de una sinfonia",
            pista(
                "Symphony No. 5 in C Minor, Op. 67: I. Allegro con brio",
                "Ludwig van Beethoven",
                None,
                448,
            ),
            vec![topic(
                "bueno",
                "Symphony No. 5 in C Minor, Op. 67: I. Allegro con brio",
                "Ludwig van Beethoven",
                450,
            )],
            Some("bueno"),
        ),
        caso(
            "blue-in-green: el bucle de una hora no es el tema",
            pista("Blue in Green", "Miles Davis", Some("Kind of Blue"), 337),
            vec![
                suelto(
                    "basura-loop",
                    "Blue in Green (1 Hour Loop)",
                    "JazzLoops",
                    3600,
                    2_000_000,
                ),
                topic("bueno", "Blue in Green", "Miles Davis", 337),
            ],
            Some("bueno"),
        ),
        // ── Electrónica y remixes legítimos ──────────────────────────────────
        caso(
            "one-more-time: youtube music frente a una resubida",
            pista("One More Time", "Daft Punk", Some("Discovery"), 320),
            vec![
                suelto(
                    "otro",
                    "Daft Punk - One More Time",
                    "ElectroFan",
                    320,
                    400_000,
                ),
                music("bueno", "One More Time", "Daft Punk", 320),
            ],
            Some("bueno"),
        ),
        caso(
            "levels-radio-edit: spotify pide el radio edit",
            pista("Levels - Radio Edit", "Avicii", None, 199),
            vec![
                topic("original", "Levels (Original Mix)", "Avicii", 320),
                topic("edit", "Levels (Radio Edit)", "Avicii", 199),
            ],
            Some("edit"),
        ),
        caso(
            "ghosts-n-stuff: un remix declarado encuentra su remix",
            pista("Ghosts 'n' Stuff - Nero Remix", "deadmau5", None, 300),
            vec![
                topic("original", "Ghosts 'n' Stuff", "deadmau5", 200),
                topic("remix", "Ghosts 'n' Stuff (Nero Remix)", "deadmau5", 300),
            ],
            Some("remix"),
        ),
        caso(
            "sandstorm-original: sin remix declarado, gana el original",
            pista("Sandstorm", "Darude", None, 225),
            vec![
                topic("remix", "Sandstorm (Extended Remix)", "Darude", 220),
                topic("original", "Sandstorm", "Darude", 225),
            ],
            Some("original"),
        ),
        caso(
            "strobe: tema largo frente a un mix de dj",
            pista("Strobe", "deadmau5", Some("For Lack of a Better Name"), 634),
            vec![
                suelto(
                    "basura-mix",
                    "deadmau5 Best Of Mix 2020",
                    "EDM Nation",
                    3600,
                    5_000_000,
                ),
                topic("bueno", "Strobe", "deadmau5", 634),
            ],
            Some("bueno"),
        ),
        caso(
            "losing-it: single sin album",
            pista("Losing It", "Fisher", None, 213),
            vec![topic("bueno", "Losing It", "Fisher", 213)],
            Some("bueno"),
        ),
        // ── Directos legítimos: la excepción del sistema ─────────────────────
        caso(
            "hotel-california-live: spotify pide el directo",
            pista(
                "Hotel California - Live",
                "Eagles",
                Some("Hell Freezes Over"),
                428,
            ),
            vec![
                topic("estudio", "Hotel California", "Eagles", 391),
                topic("live", "Hotel California (Live)", "Eagles", 428),
            ],
            Some("live"),
        ),
        caso(
            "wish-you-were-here-live",
            pista(
                "Wish You Were Here - Live",
                "Pink Floyd",
                Some("Delicate Sound of Thunder"),
                267,
            ),
            vec![
                topic("estudio", "Wish You Were Here", "Pink Floyd", 334),
                topic("live", "Wish You Were Here (Live)", "Pink Floyd", 267),
            ],
            Some("live"),
        ),
        caso(
            "bohemian-live-aid",
            pista(
                "Bohemian Rhapsody - Live at Live Aid",
                "Queen",
                Some("Live Aid"),
                260,
            ),
            vec![
                topic("estudio", "Bohemian Rhapsody", "Queen", 354),
                topic("live", "Bohemian Rhapsody (Live at Live Aid)", "Queen", 260),
            ],
            Some("live"),
        ),
        // ── Fuera del alfabeto latino ────────────────────────────────────────
        caso(
            "zenzenzense: japones",
            pista("前前前世", "RADWIMPS", Some("君の名は。"), 285),
            vec![topic("bueno", "前前前世", "RADWIMPS", 285)],
            Some("bueno"),
        ),
        caso(
            "zankoku: japones con artista en kanji",
            pista("残酷な天使のテーゼ", "高橋洋子", None, 245),
            vec![topic("bueno", "残酷な天使のテーゼ", "高橋洋子", 245)],
            Some("bueno"),
        ),
        caso(
            "coreano",
            pista("너를 만나", "폴킴", None, 240),
            vec![topic("bueno", "너를 만나", "폴킴", 240)],
            Some("bueno"),
        ),
        caso(
            "du-hast: aleman con 8d de por medio",
            pista("Du Hast", "Rammstein", Some("Sehnsucht"), 234),
            vec![
                suelto(
                    "basura-8d",
                    "Du Hast (8D AUDIO)",
                    "8D Songs",
                    234,
                    3_000_000,
                ),
                topic("bueno", "Du Hast", "Rammstein", 234),
            ],
            Some("bueno"),
        ),
        caso(
            "ne-me-quitte-pas: frances",
            pista("Ne me quitte pas", "Jacques Brel", None, 220),
            vec![topic("bueno", "Ne me quitte pas", "Jacques Brel", 220)],
            Some("bueno"),
        ),
        caso(
            "mediterraneo: acentos en castellano",
            pista("Mediterráneo", "Joan Manuel Serrat", None, 340),
            vec![topic("bueno", "Mediterraneo", "Joan Manuel Serrat", 340)],
            Some("bueno"),
        ),
        caso(
            "bailando: reggaeton con karaoke",
            pista("Bailando", "Enrique Iglesias", Some("Sex and Love"), 243),
            vec![
                suelto(
                    "basura-karaoke",
                    "Bailando (Karaoke)",
                    "Karaoke Latino",
                    243,
                    2_500_000,
                ),
                topic("bueno", "Bailando", "Enrique Iglesias", 243),
            ],
            Some("bueno"),
        ),
        caso(
            "gasolina: el remix de un dj no es la cancion",
            pista("Gasolina", "Daddy Yankee", Some("Barrio Fino"), 192),
            vec![
                suelto(
                    "basura-remix",
                    "Gasolina (DJ Remix)",
                    "DJ Mixes",
                    192,
                    4_000_000,
                ),
                topic("bueno", "Gasolina", "Daddy Yankee", 192),
            ],
            Some("bueno"),
        ),
        // ── Trampas de subcadena ─────────────────────────────────────────────
        caso(
            "cover-me-in-sunshine: 'cover' dentro del titulo legitimo",
            pista("Cover Me in Sunshine", "P!nk", None, 158),
            vec![topic("bueno", "Cover Me in Sunshine", "P!nk", 158)],
            Some("bueno"),
        ),
        caso(
            "alive: 'live' dentro de una palabra",
            pista("Alive", "Sia", Some("This Is Acting"), 261),
            vec![topic("bueno", "Alive", "Sia", 261)],
            Some("bueno"),
        ),
        caso(
            "live-and-let-die: 'live' como palabra, pero del titulo real",
            pista("Live and Let Die", "Wings", None, 193),
            vec![topic("bueno", "Live and Let Die", "Wings", 193)],
            Some("bueno"),
        ),
        caso(
            "mixed-emotions: 'mix' dentro de 'mixed'",
            pista("Mixed Emotions", "The Rolling Stones", None, 317),
            vec![topic("bueno", "Mixed Emotions", "The Rolling Stones", 317)],
            Some("bueno"),
        ),
        // ── Duraciones extremas ──────────────────────────────────────────────
        caso(
            "her-majesty: veintitres segundos",
            pista("Her Majesty", "The Beatles", Some("Abbey Road"), 23),
            vec![
                topic("largo", "Her Majesty", "The Beatles", 240),
                topic("bueno", "Her Majesty", "The Beatles", 23),
            ],
            Some("bueno"),
        ),
        caso(
            "echoes: veintitres minutos",
            pista("Echoes", "Pink Floyd", Some("Meddle"), 1413),
            vec![
                topic("corto", "Echoes (Edit)", "Pink Floyd", 420),
                topic("bueno", "Echoes", "Pink Floyd", 1413),
            ],
            Some("bueno"),
        ),
        // ── Variantes del título ─────────────────────────────────────────────
        caso(
            // A diferencia de un directo o un remix, una remasterizacion es la
            // misma grabacion: `search_title` recorta el sufijo justamente para
            // que un candidato con el titulo escueto siga sirviendo. Lo que se
            // mide aqui es que el sufijo no impida la coincidencia.
            "come-together-remaster: el sufijo editorial no rompe el emparejamiento",
            pista(
                "Come Together - Remastered 2009",
                "The Beatles",
                Some("Abbey Road"),
                259,
            ),
            vec![
                topic("bueno", "Come Together", "The Beatles", 259),
                suelto(
                    "basura-karaoke",
                    "Come Together (Karaoke)",
                    "Sing King",
                    259,
                    600_000,
                ),
            ],
            Some("bueno"),
        ),
        caso(
            "stay-con-invitado: el 'with' del titulo no debe estorbar",
            pista("Stay (with Justin Bieber)", "The Kid LAROI", None, 141),
            vec![topic("bueno", "Stay", "The Kid LAROI", 141)],
            Some("bueno"),
        ),
        caso(
            "99-problems: numeros en el titulo",
            pista("99 Problems", "JAY-Z", Some("The Black Album"), 235),
            vec![topic("bueno", "99 Problems", "JAY-Z", 235)],
            Some("bueno"),
        ),
        caso(
            "humble: puntuacion en el titulo",
            pista("HUMBLE.", "Kendrick Lamar", Some("DAMN."), 177),
            vec![
                suelto(
                    "basura-loop",
                    "HUMBLE. (1 hour loop)",
                    "LoopStation",
                    3600,
                    1_000_000,
                ),
                topic("bueno", "HUMBLE.", "Kendrick Lamar", 177),
            ],
            Some("bueno"),
        ),
        caso(
            "the-rip: una resubida sin recorrido no gana al canal oficial",
            pista("The Rip", "Portishead", Some("Third"), 268),
            vec![
                suelto("resubida", "Portishead - The Rip", "user4412", 268, 300),
                topic("bueno", "The Rip", "Portishead", 268),
            ],
            Some("bueno"),
        ),
        caso(
            "two-weeks: indie con cover de por medio",
            pista("Two Weeks", "FKA twigs", Some("LP1"), 244),
            vec![
                suelto(
                    "basura-cover",
                    "Two Weeks (Cover)",
                    "BedroomSessions",
                    244,
                    200_000,
                ),
                topic("bueno", "Two Weeks", "FKA twigs", 244),
            ],
            Some("bueno"),
        ),
        // ── Casos donde lo correcto es no descargar ──────────────────────────
        caso(
            "perfect: solo hay karaokes y covers",
            pista("Perfect", "Ed Sheeran", Some("÷"), 263),
            vec![
                suelto(
                    "basura-karaoke",
                    "Perfect (Karaoke Version)",
                    "Sing King",
                    263,
                    5_000_000,
                ),
                suelto(
                    "basura-cover",
                    "Perfect (Cover by Anna)",
                    "AnnaSings",
                    265,
                    800_000,
                ),
            ],
            None,
        ),
        caso(
            "blue-monday: solo hay un recopilatorio",
            pista(
                "Blue Monday",
                "New Order",
                Some("Power, Corruption & Lies"),
                450,
            ),
            vec![suelto(
                "basura-recopilatorio",
                "80s Hits Full Album",
                "Retro Radio",
                3600,
                9_000_000,
            )],
            None,
        ),
        caso(
            "paranoid-android: el unico candidato dura la mitad",
            pista("Paranoid Android", "Radiohead", Some("OK Computer"), 387),
            vec![topic("mitad", "Paranoid Android", "Radiohead", 190)],
            None,
        ),
        caso(
            "cancion-inexistente: nada se parece",
            pista("Tema Que No Existe", "Grupo Inventado", None, 210),
            vec![
                suelto("nada1", "Otra Cosa Distinta", "Random", 400, 40),
                suelto(
                    "nada2",
                    "Karaoke Hits Vol 7",
                    "Karaoke Channel",
                    3600,
                    1_000,
                ),
            ],
            None,
        ),
    ]
}

// ─────────────────────────────────────────────────────────────────────────────
// Medición
// ─────────────────────────────────────────────────────────────────────────────

/// Lo que el sistema descargaría de verdad: `None` si la confianza no permite
/// descarga automática, porque en ese caso no entra nada en la biblioteca.
fn elegido(caso: &Caso) -> Option<String> {
    elegir_mejor(&caso.pista, &caso.candidatos, &[])
        .filter(|r| r.confidence.permite_descarga_automatica())
        .map(|r| r.best.video_id)
}

#[test]
fn el_corpus_tiene_cincuenta_casos() {
    // El criterio de la Fase 6 fija el tamaño. Si alguien borra casos para que
    // la tasa suba, este test lo delata.
    assert_eq!(corpus().len(), 50);
}

#[test]
fn ningun_karaoke_cover_ni_8d_entra_en_la_biblioteca() {
    // El criterio duro: cero falsos positivos. Un acierto que falta se puede
    // reintentar mañana; un karaoke descargado se queda para siempre (ADR-017).
    let colados: Vec<String> = corpus()
        .iter()
        .filter_map(|c| {
            let id = elegido(c)?;
            id.starts_with("basura-")
                .then(|| format!("{}: eligio '{id}'", c.nombre))
        })
        .collect();

    assert!(
        colados.is_empty(),
        "{} candidato(s) contaminado(s) se colaron:\n{}",
        colados.len(),
        colados.join("\n")
    );
}

#[test]
fn el_corpus_supera_el_noventa_por_ciento_de_aciertos() {
    let casos = corpus();
    let fallos: Vec<String> = casos
        .iter()
        .filter_map(|c| {
            let obtenido = elegido(c);
            (obtenido.as_deref() != c.esperado).then(|| {
                format!(
                    "{}: esperado {:?}, obtenido {:?}",
                    c.nombre, c.esperado, obtenido
                )
            })
        })
        .collect();

    let aciertos = casos.len() - fallos.len();
    #[allow(clippy::cast_precision_loss, reason = "50 casos caben de sobra en f64")]
    let tasa = aciertos as f64 / casos.len() as f64;

    assert!(
        tasa >= 0.90,
        "tasa de acierto {:.0}% ({aciertos}/{}), por debajo del 90% exigido:\n{}",
        tasa * 100.0,
        casos.len(),
        fallos.join("\n")
    );
}
