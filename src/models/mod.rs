//! models 模块 — 对齐 agegr/pi-web `lib/model-discovery.ts` + `lib/provider-listing.ts`
//! + `lib/model-catalog.ts` + `lib/model-scope.ts` + `lib/models-cache.ts`
//! + `lib/model-discovery-auth.ts` + `lib/provider-credential-store.ts`。

pub mod cache;
pub mod catalog;
pub mod credential_store;
pub mod discovery;
pub mod discovery_auth;
pub mod provider_listing;
pub mod scope;

pub use cache::{
    ModelsCacheState, ModelsData, ModelListEntry, ModelRef, load_models_with_cache,
    load_models_with_cache_at, with_model_runtime_error,
};
pub use catalog::{
    ModelCatalogCost, ModelCatalogEntry, ModelCatalogMatchMethod, ModelCatalogPreset,
    ModelCatalogPriceRecommendation, ModelCatalogRecommendation, UnreliableReason,
    flatten_models_dev_catalog, recommend_model_catalog_preset, search_model_catalog,
};
pub use credential_store::{
    CredentialRemovalResult, CredentialStoreError, remove_stored_credential_if_type,
    store_provider_credential,
};
pub use discovery::{DiscoveredModel, parse_discovered_models, build_models_list_url};
pub use discovery_auth::{
    DISCOVERY_MODEL_ID, ModelDiscoveryAuth, ModelDiscoveryEngine, ResolvedAuth,
    build_discovery_models_document, discovery_temp_prefix, resolve_model_discovery_auth,
    resolve_model_discovery_auth_blocking, string_record,
};
pub use scope::{
    InitialModelScopeOptions, InitialModelScopeResult, Model, ModelScopeError, ModelScopeResult,
    ScopedModel, resolve_visible_models, select_initial_model_scope,
};
pub use provider_listing::{
    ProviderListingInput, ProviderAuthStatus, ApiKeyProviderListing, OAuthProviderListing,
    build_api_key_provider_list, build_oauth_provider_list,
};
