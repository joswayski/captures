mod conceal_policy;

pub use conceal_policy::should_conceal_documents_for_capture_activation;

#[cfg(target_os = "macos")]
mod macos_window;

#[cfg(target_os = "macos")]
pub use macos_window::*;
