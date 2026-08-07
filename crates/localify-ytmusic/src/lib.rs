//! # localify-ytmusic
//!
//! Adaptador de [`MetadataProvider`] sobre la API interna de YouTube Music.
//!
//! **Estado: en construcción.**

pub mod innertube;
pub mod parseo;
pub mod provider;

pub use provider::YtMusicProvider;
