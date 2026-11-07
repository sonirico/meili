use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use google_gmail1::yup_oauth2::authenticator_delegate::InstalledFlowDelegate;
use tokio::sync::Notify;

pub struct BrowserDelegate;

impl InstalledFlowDelegate for BrowserDelegate {
    fn present_user_url<'a>(
        &'a self,
        url: &'a str,
        _need_code: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async move {
            if let Err(e) = open::that(url) {
                eprintln!("cannot open the browser ({e}); open this URL: {url}");
            }
            Ok(String::new())
        })
    }
}

/// yup-oauth2 ignores the delegate's result and keeps waiting for the redirect,
/// so the refusal is signalled through `Notify` and the caller races it.
pub struct NoLoginDelegate(pub Arc<Notify>);

impl InstalledFlowDelegate for NoLoginDelegate {
    fn present_user_url<'a>(
        &'a self,
        _url: &'a str,
        _need_code: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String, String>> + Send + 'a>> {
        Box::pin(async {
            self.0.notify_one();
            Err("not logged in".to_string())
        })
    }
}
