//! models 模块 — 对齐 agegr/pi-web `lib/model-discovery.ts` + `lib/provider-listing.ts`
//! + `lib/model-catalog.ts` + `lib/model-scope.ts` + `lib/models-cache.ts`。

pub mod cache;
pub mod catalog;
pub mod discovery;
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
pub use discovery::{DiscoveredModel, parse_discovered_models, build_models_list_url};
pub use scope::{
    InitialModelScopeOptions, InitialModelScopeResult, Model, ModelScopeError, ModelScopeResult,
    ScopedModel, resolve_visible_models, select_initial_model_scope,
};
pub use provider_listing::{
    ProviderListingInput, ProviderAuthStatus, ApiKeyProviderListing, OAuthProviderListing,
    build_api_key_provider_list, build_oauth_provider_list,
};
