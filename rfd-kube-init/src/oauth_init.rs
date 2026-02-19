// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use anyhow::{anyhow, Context, Result};
use clap::Args;
use serde::{Deserialize, Serialize};

use crate::kube;

/// Request body for the /init endpoint
#[derive(Debug, Serialize)]
struct InitRequest {
    redirect_uris: Vec<String>,
}

/// Response from the /init endpoint
#[derive(Debug, Deserialize)]
struct InitResponse {
    client_id: String,
    secret: String,
    redirect_uris: Vec<String>,
}

#[derive(Args)]
pub struct OAuthInitArgs {
    /// RFD API host URL (e.g., http://rfd-api:8080)
    #[arg(long, env = "RFD_API_HOST")]
    host: String,

    /// Redirect URIs for the OAuth client (comma-separated)
    #[arg(long, env = "OAUTH_REDIRECT_URIS", value_delimiter = ',')]
    redirect_uris: Vec<String>,

    /// Target namespaces to write the OAuth client credentials (comma-separated)
    #[arg(long, env = "OAUTH_TARGET_NAMESPACES", value_delimiter = ',')]
    target_namespaces: Vec<String>,

    /// Name of the secret to create in target namespaces
    #[arg(long, env = "OAUTH_SECRET_NAME", default_value = "rfd-oauth-client")]
    secret_name: String,
}

struct FailedNamespace {
    namespace: String,
    error: String,
}

/// Initialize OAuth client and distribute credentials to target namespaces.
pub async fn init(kube_client: &::kube::Client, args: &OAuthInitArgs) -> Result<()> {
    if args.redirect_uris.is_empty() {
        return Err(anyhow!("At least one redirect URI must be provided"));
    }

    if args.target_namespaces.is_empty() {
        return Err(anyhow!("At least one target namespace must be provided"));
    }

    tracing::info!(
        host = %args.host,
        redirect_uri_count = args.redirect_uris.len(),
        target_namespace_count = args.target_namespaces.len(),
        "Initializing OAuth client"
    );

    // Call the /init endpoint
    let client = reqwest::Client::new();
    let init_url = format!("{}/init", args.host.trim_end_matches('/'));

    let request_body = InitRequest {
        redirect_uris: args.redirect_uris.clone(),
    };

    tracing::debug!(url = %init_url, "Calling /init endpoint");

    let response = client
        .post(&init_url)
        .json(&request_body)
        .send()
        .await
        .context("Failed to send request to /init endpoint")?;

    let status = response.status();

    let init_response: InitResponse = match status {
        reqwest::StatusCode::CONFLICT => {
            tracing::warn!("System already initialized (409 Conflict), skipping");
            return Ok(());
        }
        s if s.is_success() => response
            .json()
            .await
            .context("Failed to parse /init response")?,
        _ => {
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            tracing::error!(status = %status, error = %error_text, "Failed to initialize OAuth client");
            return Err(anyhow!(
                "Failed to initialize OAuth client: {} - {}",
                status,
                error_text
            ));
        }
    };

    tracing::info!(
        client_id = %init_response.client_id,
        redirect_uri_count = init_response.redirect_uris.len(),
        "OAuth client created successfully"
    );

    // Distribute credentials to target namespaces
    let mut failures = Vec::new();

    for ns in &args.target_namespaces {
        match kube::write_secret(
            kube_client,
            ns,
            &args.secret_name,
            &[
                ("OAUTH_CLIENT_ID", &init_response.client_id),
                ("OAUTH_CLIENT_SECRET", &init_response.secret),
            ],
        )
        .await
        {
            Ok(()) => {
                tracing::info!(
                    namespace = ns.as_str(),
                    secret = args.secret_name.as_str(),
                    "Wrote OAuth client credentials"
                );
            }
            Err(err) => {
                failures.push(FailedNamespace {
                    namespace: ns.clone(),
                    error: err.to_string(),
                });
                tracing::error!(
                    namespace = ns.as_str(),
                    secret = args.secret_name.as_str(),
                    error = %err,
                    "Failed to write OAuth client credentials"
                );
            }
        }
    }

    if !failures.is_empty() {
        let failed_list: Vec<String> = failures
            .iter()
            .map(|f| format!("{}({})", f.namespace, f.error))
            .collect();
        tracing::error!(
            failed_count = failures.len(),
            failed_namespaces = %failed_list.join(", "),
            "Failed to write secrets to some namespaces"
        );
        return Err(anyhow!(
            "Failed to write secrets to namespaces: {}",
            failures.iter().map(|f| f.namespace.as_str()).collect::<Vec<_>>().join(", ")
        ));
    }

    tracing::info!(
        namespace_count = args.target_namespaces.len(),
        "OAuth client credentials distributed successfully"
    );

    Ok(())
}
