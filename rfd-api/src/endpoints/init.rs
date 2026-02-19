// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use chrono::Utc;
use dropshot::{
    endpoint, ClientErrorStatusCode, HttpError, HttpResponseCreated, RequestContext, TypedBody,
};
use newtype_uuid::{GenericUuid, TypedUuid};
use rfd_model::{storage::InitializationStore, InitializationModel};
use schemars::JsonSchema;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use trace_request::trace_request;
use tracing::instrument;
use uuid::Uuid;
use v_api::{
    authn::key::RawKey,
    response::{client_error, to_internal_error},
    ApiContext,
};
use v_model::{
    storage::{OAuthClientRedirectUriStore, OAuthClientSecretStore, OAuthClientStore},
    NewOAuthClient, NewOAuthClientRedirectUri, NewOAuthClientSecret, OAuthClientId,
};

use crate::context::RfdContext;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InitRequestBody {
    pub redirect_uris: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct InitResponse {
    pub client_id: TypedUuid<OAuthClientId>,
    pub secret: String,
    pub redirect_uris: Vec<String>,
}

/// Initialize the system with an initial OAuth client.
///
/// This endpoint is unauthenticated and can only be called once. If the system
/// has already been initialized, this endpoint will return a 409 Conflict error.
///
/// To re-initialize the system, an administrator must manually delete the
/// initialization record from the database.
#[trace_request]
#[endpoint {
    method = POST,
    path = "/init",
    tags = ["hidden"],
}]
#[instrument(skip(rqctx), fields(request_id = rqctx.request_id), err(Debug))]
pub async fn init(
    rqctx: RequestContext<RfdContext>,
    body: TypedBody<InitRequestBody>,
) -> Result<HttpResponseCreated<InitResponse>, HttpError> {
    let ctx = rqctx.context();
    let body = body.into_inner();
    init_op(ctx, body).await
}

/// Internal operation for system initialization, separated for testability.
#[instrument(skip(ctx), err(Debug))]
pub async fn init_op(
    ctx: &RfdContext,
    body: InitRequestBody,
) -> Result<HttpResponseCreated<InitResponse>, HttpError> {
    // Step 1: Check if initialization record already exists
    let existing = InitializationStore::get(&*ctx.storage)
        .await
        .map_err(to_internal_error)?;

    if existing.is_some() {
        return Err(client_error(
            ClientErrorStatusCode::CONFLICT,
            "System already initialized",
        ));
    }

    // Step 2: Create the OAuth client directly using storage (bypassing permission checks)
    // This is safe because we've already verified this is the first initialization via the
    // InitializationStore check above.
    let client_id = TypedUuid::new_v4();
    let client = OAuthClientStore::upsert(ctx.v_storage(), NewOAuthClient { id: client_id })
        .await
        .map_err(|e| {
            tracing::error!(?e, "Failed to create OAuth client");
            to_internal_error(e)
        })?;

    // Step 3: Create a secret for the client
    let secret_id = TypedUuid::new_v4();
    let secret = RawKey::generate::<24>(secret_id.as_untyped_uuid())
        .sign(ctx.v_ctx().signer())
        .await
        .map_err(|e| {
            tracing::error!(?e, "Failed to sign OAuth client secret");
            to_internal_error(e)
        })?;

    OAuthClientSecretStore::upsert(
        ctx.v_storage(),
        NewOAuthClientSecret {
            id: secret_id,
            oauth_client_id: client.id,
            secret_signature: secret.signature().to_string(),
        },
    )
    .await
    .map_err(|e| {
        tracing::error!(?e, "Failed to store OAuth client secret");
        to_internal_error(e)
    })?;

    // Step 4: Add all redirect URIs
    for redirect_uri in &body.redirect_uris {
        OAuthClientRedirectUriStore::upsert(
            ctx.v_storage(),
            NewOAuthClientRedirectUri {
                id: TypedUuid::new_v4(),
                oauth_client_id: client.id,
                redirect_uri: redirect_uri.clone(),
            },
        )
        .await
        .map_err(|e| {
            tracing::error!(?e, ?redirect_uri, "Failed to add redirect URI");
            to_internal_error(e)
        })?;
    }

    // Step 5: Write the initialization record
    let init_record = InitializationModel {
        id: Uuid::new_v4(),
        initialized_at: Utc::now(),
        oauth_client_id: client.id.into_untyped_uuid(),
    };

    InitializationStore::insert(&*ctx.storage, init_record)
        .await
        .map_err(|e| {
            tracing::error!(?e, "Failed to insert initialization record");
            to_internal_error(e)
        })?;

    tracing::info!(
        client_id = %client.id,
        "System initialized successfully"
    );

    Ok(HttpResponseCreated(InitResponse {
        client_id: client.id,
        secret: secret.key().expose_secret().to_string(),
        redirect_uris: body.redirect_uris,
    }))
}

