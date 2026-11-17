//! The browser half of an OAuth sign-in, shared by ChatGPT and sources: a
//! one-time listener on 127.0.0.1 that the provider sends the browser back to.

use std::time::{Duration, Instant};

use tiny_http::{Header, Method, Response, Server};
use url::Url;

use crate::Result;

const TIMEOUT: Duration = Duration::from_secs(180);

pub struct Callback {
    server: Server,
    path: &'static str,
    /// The `redirect_uri` to send the provider.
    pub redirect: String,
}

impl Callback {
    /// Listens on a free port for a redirect to `path`, such as `/callback`.
    pub fn listen(path: &'static str) -> Result<Self> {
        let server = Server::http("127.0.0.1:0")?;
        let redirect = format!("http://{}{path}", server.server_addr());
        Ok(Self {
            server,
            path,
            redirect,
        })
    }

    /// Opens `authorize` in the browser and waits for the redirect that
    /// carries `state`. `finish` turns that redirect into the outcome, and the
    /// browser tab then says whether `name` connected.
    pub fn wait<T>(
        self,
        authorize: &Url,
        state: &str,
        name: &str,
        finish: impl FnOnce(&Url) -> Result<T>,
    ) -> Result<T> {
        open::that_detached(authorize.as_str())
            .map_err(|_| format!("Context could not open {name} sign-in in your browser."))?;
        let base = Url::parse(&self.redirect)?;
        let deadline = Instant::now() + TIMEOUT;
        while let Some(incoming) = self
            .server
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))?
        {
            let callback = base.join(incoming.url()).ok().filter(|url| {
                incoming.method() == &Method::Get
                    && url.origin() == base.origin()
                    && url.path() == self.path
                    && parameter(url, "state").as_deref() == Some(state)
            });
            let Some(callback) = callback else {
                let _ = incoming.respond(
                    Response::from_string("This request does not match the Context sign-in.")
                        .with_status_code(400),
                );
                continue;
            };
            let outcome = finish(&callback);
            let message = match &outcome {
                Ok(_) => format!("{name} is connected to Context. You can close this tab."),
                Err(_) => {
                    "Context could not finish signing in. Check the Context window for details."
                        .into()
                }
            };
            let _ = incoming.respond(page(&message, outcome.is_ok()));
            return outcome;
        }
        Err(format!("{name} sign-in timed out. Try again from Context.").into())
    }
}

/// A query parameter that appears exactly once and is not empty.
pub fn parameter(url: &Url, name: &str) -> Option<String> {
    let mut values = url.query_pairs().filter(|(key, _)| key == name);
    let value = values.next()?.1.into_owned();
    (values.next().is_none() && !value.is_empty()).then_some(value)
}

/// A small page for the browser tab that finished sign-in.
fn page(message: &str, connected: bool) -> Response<std::io::Cursor<Vec<u8>>> {
    let mark = if connected { "&#10003;" } else { "!" };
    let message = message.replace('&', "&amp;").replace('<', "&lt;");
    let html = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width'>\
         <title>Context</title><style>body{{font:16px system-ui,sans-serif;display:grid;\
         place-items:center;min-height:90vh;margin:0;background:#f6f7f9;color:#15181d}}\
         @media(prefers-color-scheme:dark){{body{{background:#111317;color:#eceff4}}}}\
         div{{text-align:center;max-width:26rem;padding:1rem}}b{{display:block;font-size:2rem;\
         margin-bottom:.5rem;color:#5b74f0}}</style><div><b>{mark}</b>{message}</div>"
    );
    let header = Header::from_bytes("Content-Type", "text/html; charset=utf-8")
        .expect("static header is valid");
    Response::from_data(html.into_bytes()).with_header(header)
}
