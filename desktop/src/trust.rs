//! Per-webview trust for the exact certificate delivered on the owned sidecar's
//! private pipe. No keychain edits, default-accept challenges, or public bypass.
use objc2::{
    define_class, msg_send,
    rc::{Retained, Weak},
    runtime::{AnyObject, NSObject, ProtocolObject, Sel},
    DefinedClass, MainThreadOnly,
};
use objc2_core_foundation::{CFArray, CFString};
use objc2_foundation::{
    MainThreadMarker, NSObjectProtocol, NSURLAuthenticationChallenge, NSURLCredential,
    NSURLSessionAuthChallengeDisposition,
};
use objc2_security::{SecPolicy, SecTrust};
use objc2_web_kit::{WKNavigationDelegate, WKWebView};

struct Pin {
    original: Weak<ProtocolObject<dyn WKNavigationDelegate>>,
    port: u16,
    certificate: Vec<u8>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = Pin]
    struct PinnedNavigationDelegate;

    unsafe impl NSObjectProtocol for PinnedNavigationDelegate {
        #[unsafe(method(respondsToSelector:))]
        fn responds(&self, selector: Sel) -> bool {
            // WebKit asks which optional delegate callbacks exist. Keep Wry's
            // navigation, new-window, download and lifecycle callbacks intact.
            (unsafe { msg_send![super(self), respondsToSelector: selector] })
                || self.ivars().original.load().is_some_and(|d| d.respondsToSelector(selector))
        }
    }

    impl PinnedNavigationDelegate {
        #[unsafe(method(forwardingTargetForSelector:))]
        fn forward(&self, selector: Sel) -> *mut AnyObject {
            self.ivars().original.load()
                .filter(|d| d.respondsToSelector(selector))
                // Wry owns this delegate for the webview's lifetime, and every
                // callback runs on the main thread. A weak reference avoids a
                // webview -> proxy -> original -> webview retain cycle.
                .map_or(std::ptr::null_mut(), |d| Retained::as_ptr(&d).cast_mut().cast())
        }
    }

    unsafe impl WKNavigationDelegate for PinnedNavigationDelegate {
        #[unsafe(method(webView:didReceiveAuthenticationChallenge:completionHandler:))]
        fn challenge(&self, _view: &WKWebView, challenge: &NSURLAuthenticationChallenge,
            completion: &block2::DynBlock<dyn Fn(NSURLSessionAuthChallengeDisposition, *mut NSURLCredential)>) {
            // SAFETY: Foundation owns the challenge and its SecTrust for this
            // callback. These selectors are declared in NSURLProtectionSpace.h
            // and NSURLCredential.h; generated bindings omit their CF bridge.
            unsafe {
                let space = challenge.protectionSpace();
                let trust: *const SecTrust = msg_send![&space, serverTrust];
                let accepted = space.authenticationMethod().to_string() == "NSURLAuthenticationMethodServerTrust"
                    && space.host().to_string() == "127.0.0.1"
                    && space.port() == self.ivars().port as isize
                    && space.protocol().is_some_and(|p| p.to_string() == "https")
                    && trust.as_ref().is_some_and(|trust| accepts(trust, &self.ivars().certificate));
                if accepted {
                    let credential: Retained<NSURLCredential> = msg_send![objc2::class!(NSURLCredential), credentialForTrust: trust];
                    completion.call((NSURLSessionAuthChallengeDisposition::UseCredential, Retained::as_ptr(&credential).cast_mut()));
                } else {
                    completion.call((NSURLSessionAuthChallengeDisposition::CancelAuthenticationChallenge, std::ptr::null_mut()));
                }
            }
        }
    }
);

unsafe fn accepts(trust: &SecTrust, expected: &[u8]) -> bool {
    // Evaluate only the supplied peer, with one explicit anchor and SSL host
    // policy. Pinning is in addition to platform validation, not a bypass.
    #[allow(deprecated)]
    let Some(leaf) = (unsafe { trust.certificate_at_index(0) }) else {
        return false;
    };
    if unsafe { leaf.data() }.to_vec() != expected {
        return false;
    }
    let anchors = CFArray::from_objects(&[&*leaf]);
    let policy = unsafe { SecPolicy::new_ssl(true, Some(&CFString::from_str("127.0.0.1"))) };
    unsafe {
        trust.set_network_fetch_allowed(false) == 0
            && trust.set_anchor_certificates(Some(anchors.as_ref())) == 0
            && trust.set_anchor_certificates_only(true) == 0
            && trust.set_policies(policy.as_ref()) == 0
            && trust.evaluate_with_error(std::ptr::null_mut())
    }
}

pub fn install(window: &tauri::WebviewWindow, origin: &str, certificate: Vec<u8>) -> Result<(), String> {
    let url = tauri::Url::parse(origin).map_err(|_| "Invalid owned TLS origin.")?;
    if url.scheme() != "https" || url.host_str() != Some("127.0.0.1") || certificate.is_empty() {
        return Err("Missing owned TLS origin or certificate.".into());
    }
    let port = url.port_or_known_default().ok_or("Missing owned port.")?;
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    window
        .with_webview(move |platform| {
            let result = (|| {
                let mtm = MainThreadMarker::new().ok_or("Native trust must run on the main thread.")?;
                // SAFETY: Tauri's macOS PlatformWebview contains this live WKWebView.
                unsafe {
                    let view = &*platform.inner().cast::<WKWebView>();
                    let original = view.navigationDelegate().ok_or("Missing native navigation delegate.")?;
                    let delegate = mtm.alloc::<PinnedNavigationDelegate>().set_ivars(Pin {
                        original: Weak::from_retained(&original),
                        port,
                        certificate,
                    });
                    let delegate: Retained<PinnedNavigationDelegate> = msg_send![super(delegate), init];
                    // WKNavigationDelegate is weak. Associate the proxy with this
                    // one webview so it is released when the view is destroyed.
                    static ASSOCIATION: u8 = 0;
                    objc2::ffi::objc_setAssociatedObject(
                        (view as *const WKWebView).cast_mut().cast(),
                        (&ASSOCIATION as *const u8).cast(),
                        Retained::as_ptr(&delegate).cast_mut().cast(),
                        objc2::ffi::OBJC_ASSOCIATION_RETAIN_NONATOMIC,
                    );
                    view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
                }
                Ok(())
            })();
            let _ = send.send(result);
        })
        .map_err(|_| "Cannot install native certificate pin.")?;
    receive
        .recv_timeout(std::time::Duration::from_secs(5))
        .map_err(|_| "Native certificate pin timed out.")?
        .map_err(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_core_foundation::{CFData, CFRetained};
    use objc2_security::SecCertificate;
    use std::ptr::NonNull;

    fn certificate(host: &str, expired: bool) -> Vec<u8> {
        let mut params = rcgen::CertificateParams::new(vec![host.into()]).unwrap();
        if expired {
            params.not_before = rcgen::date_time_ymd(2020, 1, 1);
            params.not_after = rcgen::date_time_ymd(2020, 1, 2);
        }
        params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
        params
            .self_signed(&rcgen::KeyPair::generate().unwrap())
            .unwrap()
            .der()
            .to_vec()
    }

    fn check(der: &[u8], expected: &[u8]) -> bool {
        unsafe {
            let leaf = SecCertificate::with_data(None, &CFData::from_bytes(der)).unwrap();
            let policy = SecPolicy::new_ssl(true, Some(&CFString::from_str("127.0.0.1")));
            let mut trust = std::ptr::null_mut();
            assert_eq!(
                SecTrust::create_with_certificates(leaf.as_ref(), Some(policy.as_ref()), NonNull::from(&mut trust)),
                0
            );
            let trust = CFRetained::from_raw(NonNull::new(trust).unwrap());
            accepts(&trust, expected)
        }
    }

    #[test]
    fn platform_validates_exact_owned_leaf_name_and_expiry() {
        let valid = certificate("127.0.0.1", false);
        assert!(check(&valid, &valid));
        let other = certificate("127.0.0.1", false);
        assert!(!check(&valid, &other));
        let wrong_name = certificate("example.com", false);
        assert!(!check(&wrong_name, &wrong_name));
        let expired = certificate("127.0.0.1", true);
        assert!(!check(&expired, &expired));
    }
}
