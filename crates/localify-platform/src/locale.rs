//! Detección del idioma del sistema, para elegir uno en el primer arranque.

use localify_core::ports::platform::LocaleProvider;

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemLocale;

impl SystemLocale {
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl LocaleProvider for SystemLocale {
    fn system_locale(&self) -> String {
        detectar().unwrap_or_else(|| "en-US".to_owned())
    }
}

#[cfg(windows)]
fn detectar() -> Option<String> {
    use windows::Win32::Globalization::GetUserDefaultLocaleName;

    // `LOCALE_NAME_MAX_LENGTH` no está expuesto por el crate `windows`; es una
    // constante fija de la API de Windows desde Vista.
    const LOCALE_NAME_MAX_LENGTH: usize = 85;

    let mut buffer = [0_u16; LOCALE_NAME_MAX_LENGTH];
    // SAFETY: `buffer` tiene el tamaño máximo que documenta la API y vive
    // durante toda la llamada.
    let escritos = unsafe { GetUserDefaultLocaleName(&mut buffer) };
    if escritos <= 0 {
        return None;
    }

    // El valor devuelto incluye el terminador nulo. El signo ya se descartó
    // arriba, así que la conversión es segura.
    let longitud = usize::try_from(escritos).unwrap_or(1).saturating_sub(1);
    Some(String::from_utf16_lossy(&buffer[..longitud]))
}

#[cfg(not(windows))]
fn detectar() -> Option<String> {
    std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .ok()
        .map(|s| s.split('.').next().unwrap_or(&s).to_owned())
        .filter(|s| !s.is_empty() && s != "C" && s != "POSIX")
}

#[cfg(test)]
mod tests {
    use super::*;
    use localify_core::domain::settings::Language;

    #[test]
    fn el_locale_del_sistema_es_utilizable() {
        let locale = SystemLocale::new().system_locale();
        assert!(!locale.is_empty());
        // Sea cual sea el sistema, debe poder mapearse a un idioma soportado
        // sin fallar: los no soportados caen en inglés.
        let _: Language = Language::from_locale(&locale);
    }
}
