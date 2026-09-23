/// Watch-only live view link. Compose sets TABOOM_VIEW_URL from the published port.
pub fn watch_url() -> String {
    std::env::var("TABOOM_VIEW_URL")
        .unwrap_or_else(|_| "http://localhost:6080/vnc.html?autoconnect=1&resize=scale&view_only=1".into())
}

/// The same live view with mouse and keyboard enabled.
pub fn takeover_url() -> String {
    watch_url().replace("&view_only=1", "").replace("view_only=1&", "")
}
