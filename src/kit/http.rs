//! TLS settings shared by every HTTP client (model download, translation).
//! On Windows ureq uses Schannel, and with Schannel it *panics* unless the root certificates are set to the
//! platform's — which in a release build (panic = abort) closes the app. Every agent must use this.

pub fn tls() -> ureq::tls::TlsConfig {
    #[cfg(windows)]
    {
        ureq::tls::TlsConfig::builder()
            .provider(ureq::tls::TlsProvider::NativeTls)
            .root_certs(ureq::tls::RootCerts::PlatformVerifier)
            .build()
    }
    #[cfg(not(windows))]
    {
        ureq::tls::TlsConfig::default()
    }
}
