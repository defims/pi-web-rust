//! models 模块 — 对齐 agegr/pi-web `lib/model-discovery.ts` + `lib/provider-listing.ts`
//! + `lib/model-catalog.ts` + `lib/model-scope.ts` + `lib/models-cache.ts`
//! + `lib/model-discovery-auth.ts` + `lib/provider-credential-store.ts`
//! + `lib/provider-listing-runtime.ts`。

pub mod cache;
pub mod catalog;
pub mod credential_store;
pub mod discovery;
pub mod discovery_auth;
pub mod provider_listing;
pub mod provider_listing_runtime;
pub mod scope;

pub use cache::{
    load_models_with_cache, load_models_with_cache_at, with_model_runtime_error, ModelListEntry,
    ModelRef, ModelsCacheState, ModelsData,
};
pub use catalog::{
    flatten_models_dev_catalog, recommend_model_catalog_preset, search_model_catalog,
    ModelCatalogCost, ModelCatalogEntry, ModelCatalogMatchMethod, ModelCatalogPreset,
    ModelCatalogPriceRecommendation, ModelCatalogRecommendation, UnreliableReason,
};
pub use credential_store::{
    remove_stored_credential_if_type, store_provider_credential, CredentialRemovalResult,
    CredentialStoreError,
};
pub use discovery::{build_models_list_url, parse_discovered_models, DiscoveredModel};
pub use discovery_auth::{
    build_discovery_models_document, discovery_temp_prefix, resolve_model_discovery_auth,
    resolve_model_discovery_auth_blocking, string_record, ModelDiscoveryAuth, ModelDiscoveryEngine,
    ResolvedAuth, DISCOVERY_MODEL_ID,
};
pub use provider_listing::{
    build_api_key_provider_list, build_oauth_provider_list, ApiKeyProviderListing,
    OAuthProviderListing, ProviderAuthStatus, ProviderListingInput,
};
pub use provider_listing_runtime::{
    collect_provider_listing_inputs, AuthStatus, ProviderAuthDecl, ProviderRuntime,
};
pub use scope::{
    resolve_visible_models, select_initial_model_scope, InitialModelScopeOptions,
    InitialModelScopeResult, Model, ModelScopeError, ModelScopeResult, ScopedModel,
};
