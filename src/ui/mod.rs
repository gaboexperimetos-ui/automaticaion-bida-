//! Shared user-interface primitives for Study Platform.
//!
//! The Stitch "Synthetic Cognition" design system (Tailwind token set, base
//! CSS, shared `<head>` and HTML escapers) is defined once and reused by every
//! handler so the palette/typography never drifts and no CSS block is
//! duplicated per handler.
pub mod stitch;
