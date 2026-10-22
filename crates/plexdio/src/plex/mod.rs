//! Plex API: plex.tv (auth, resources) and Plex Media Server (library, playback reporting).

pub mod auth;
pub mod http;
pub mod models;
pub mod resources;
pub mod server;

pub use http::{ClientInfo, Http};

#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum PlexError {
    #[error("network error: {0}")]
    Network(String),
    #[error("not authorized (token missing or expired)")]
    Unauthorized,
    #[error("not found")]
    NotFound,
    #[error("server returned HTTP {0}")]
    Http(u16),
    #[error("unexpected response: {0}")]
    Parse(String),
    #[error("timed out")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
}

impl PlexError {
    /// Worth retrying with backoff (transient network/server trouble).
    pub fn is_transient(&self) -> bool {
        match self {
            PlexError::Network(_) | PlexError::Timeout => true,
            PlexError::Http(s) => *s == 429 || *s >= 500,
            _ => false,
        }
    }

    /// Short message suitable for the UI.
    pub fn user_message(&self) -> String {
        match self {
            PlexError::Network(_) | PlexError::Timeout => "Can't reach the server. Check your connection.".into(),
            PlexError::Unauthorized => "Your Plex sign-in has expired. Please sign in again.".into(),
            PlexError::NotFound => "The item no longer exists on the server.".into(),
            PlexError::Http(s) => format!("The server returned an error (HTTP {s})."),
            PlexError::Parse(_) => "The server sent an unexpected response.".into(),
            PlexError::Cancelled => "Cancelled.".into(),
        }
    }
}

pub type Result<T> = std::result::Result<T, PlexError>;
