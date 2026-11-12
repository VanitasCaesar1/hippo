//! hippo-feed: the community site for packs, ranked by evidence.
//!
//! One binary, `hippo-feed`, serves server-rendered HTML with no JavaScript
//! and a small read-only API, from one SQLite file.

pub mod activity;
pub mod api;
pub mod audit;
pub mod auth;
pub mod backup;
pub mod clock;
pub mod comments;
pub mod config;
pub mod db;
pub mod deeplink;
pub mod demo;
pub mod error;
pub mod evidence;
pub mod http;
pub mod jobs;
pub mod limits;
pub mod markdown;
pub mod measurements;
pub mod moderation;
pub mod page;
pub mod policy;
pub mod posts;
pub mod rank;
pub mod registry;
pub mod reports;
pub mod rings;
pub mod text;
pub mod themes;
pub mod users;
pub mod votes;
pub mod web;

pub use web::{router, AppState};
