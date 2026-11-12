//! The data behind Hippo.app's panels, with no window code.
//!
//! Each module reads hippo's files and index through `hippo-core` and returns
//! a plain value that the app draws. Nothing here writes a file, starts a
//! thread or links GPUI, so the engine's CI builds and tests it on Linux as
//! well as macOS. Functions block; the app calls them on a background
//! executor.
//!
//! [`setup`] (is hippo set up), [`status`] (the status bar), [`fuzzy`] (the
//! finder's matcher) and [`text`] (escaping for text that comes from files)
//! serve the shell; [`brain`], [`sessions`] and [`dreaming`] are the panels'
//! models, [`digest`] reads what a dreaming run did from its report, [`edition`] makes the home's daily edition from the rollup, [`notepage`] a note's page (sections, links in and out, dreaming, readers), and [`finder`] makes the finder's rows from the first two.
//! [`worddiff`] finds what a dreaming change changed, word by word.
//! [`conversation`] and [`markdown`] turn a session's turns into what its
//! tab draws.

pub mod brain;
pub mod briefing;
pub mod changes;
pub mod conversation;
pub mod correction;
pub mod digest;
pub mod dreaming;
pub mod edition;
pub mod finder;
pub mod fleet;
pub mod fuzzy;
pub mod home;
pub mod manage;
pub mod markdown;
pub mod memory;
pub mod notepage;
pub mod onboarding;
pub mod replay;
pub mod sessions;
pub mod setup;
pub mod status;
pub mod text;
pub mod worddiff;
