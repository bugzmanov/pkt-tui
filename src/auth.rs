use anyhow::Context;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{io::prelude::*, net::TcpListener, sync::mpsc, thread, time::Duration};
use tokio::runtime::Runtime;

use crate::pocket::CONSUMER_KEY;

#[derive(Debug)]
pub struct PocketAuth {
    client: Client,
    runtime: Runtime,
}

#[derive(Deserialize, Debug)]
struct RequestTokenResponse {
    code: String,
}

#[derive(Deserialize, Debug)]
struct AccessTokenResponse {
    access_token: String,
    _username: String, //todo: use username to store user dedicated snapshots
}

#[derive(Serialize)]
struct RequestTokenPayload<'a> {
    consumer_key: &'a str,
    redirect_uri: &'a str,
}

#[derive(Serialize)]
struct AccessTokenPayload<'a> {
    consumer_key: &'a str,
    code: &'a str,
}

struct ServerInfo {
    port: u16,
    receiver: mpsc::Receiver<()>,
}

impl PocketAuth {
    pub fn new() -> anyhow::Result<Self> {
        Ok(PocketAuth {
            client: Client::new(),
            runtime: Runtime::new().context("Failed to create Tokio runtime")?,
        })
    }

    /// Start a local HTTP server with a random port
    fn start_callback_server(&self) -> anyhow::Result<ServerInfo> {
        // Bind to port 0 lets the system assign a random available port
        let listener =
            TcpListener::bind("127.0.0.1:0").context("Failed to start callback server")?;

        // Get the assigned port
        let port = listener
            .local_addr()
            .context("Failed to get local address")?
            .port();

        let (tx, rx) = mpsc::channel();

        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0; 1024];
                if stream.read(&mut buffer).is_ok() {
                    // Send success response to browser
                    let response = "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\n\r\n\
                        <html><body><h1>Authorization Successful!</h1>\
                        <p>You can close this window and return to the application.</p></body></html>";

                    if let Err(e) = stream.write_all(response.as_bytes()) {
                        eprintln!("Failed to send response to browser: {}", e);
                    }
                }
                // Signal that we received the callback
                let _ = tx.send(());
            }
        });

        Ok(ServerInfo { port, receiver: rx })
    }

    /// Request a request token from Pocket API
    async fn get_request_token(&self, redirect_uri: &str) -> anyhow::Result<String> {
        let payload = RequestTokenPayload {
            consumer_key: CONSUMER_KEY,
            redirect_uri,
        };

        let response = self
            .client
            .post("https://getpocket.com/v3/oauth/request")
            .header("X-Accept", "application/json")
            .json(&payload)
            .send()
            .await
            .context("Failed to send request token request")?;

        let token_response = response
            .json::<RequestTokenResponse>()
            .await
            .context("Failed to parse request token response")?;

        Ok(token_response.code)
    }

    /// Generate the authorization URL for the user
    fn get_authorization_url(&self, request_token: &str, redirect_uri: &str) -> String {
        format!(
            "https://getpocket.com/auth/authorize?request_token={}&redirect_uri={}",
            request_token, redirect_uri
        )
    }

    /// Convert request token to access token
    async fn get_access_token(&self, request_token: &str) -> anyhow::Result<String> {
        let payload = AccessTokenPayload {
            consumer_key: CONSUMER_KEY,
            code: request_token,
        };

        let response = self
            .client
            .post("https://getpocket.com/v3/oauth/authorize")
            .header("X-Accept", "application/json")
            .json(&payload)
            .send()
            .await
            .context("Failed to send access token request")?;

        let token_response = response
            .json::<AccessTokenResponse>()
            .await
            .context("Failed to parse access token response")?;

        Ok(token_response.access_token)
    }

    /// Complete authentication flow
    pub fn authenticate(&self) -> anyhow::Result<String> {
        // Start the callback server with random port
        let ServerInfo {
            port,
            receiver: callback_receiver,
        } = self.start_callback_server()?;

        let redirect_uri = format!("http://localhost:{}/callback", port);
        println!("Using callback URL: {}", redirect_uri);

        // Use the runtime to execute async authentication flow
        self.runtime.block_on(async {
            // Get request token
            let request_token = self.get_request_token(&redirect_uri).await?;

            // Get authorization URL and open it in browser
            let auth_url = self.get_authorization_url(&request_token, &redirect_uri);
            webbrowser::open(&auth_url).context("Failed to open authorization URL in browser")?;

            println!("Waiting for authorization...");

            // Wait for callback with ctrl-c handling
            let result = tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    anyhow::bail!("Authentication cancelled by user. Terminating")
                }
                result = async {
                    match callback_receiver.recv_timeout(Duration::from_secs(300)) {
                        Ok(_) => {
                            // Small delay to ensure Pocket has processed the authorization
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            // Get access token
                            self.get_access_token(&request_token).await
                        }
                        Err(_) => anyhow::bail!("Authorization timeout - no response received"),
                    }
                } => result
            };
            result
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    //todo: move to integration tests
    #[test]
    fn test_auth_url_generation() {
        let auth = PocketAuth::new().unwrap();
        let request_token = "test_token";
        let redirect_uri = "http://localhost:12345/callback";
        let url = auth.get_authorization_url(request_token, redirect_uri);
        assert!(url.contains(request_token));
        assert!(url.contains(redirect_uri));
    }
}
