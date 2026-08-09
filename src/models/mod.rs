//! models 模块 — 对齐 agegr/pi-web `lib/model-discovery.ts` + `lib/provider-listing.ts`
//! + `lib/model-catalog.ts`。

pub mod catalog;
pub mod discovery;
pub mod provider_listing;

pub use catalog::{
    ModelCatalogCost, ModelCatalogEntry, ModelCatalogMatchMethod, ModelCatalogPreset,
    ModelCatalogPriceRecommendation, ModelCatalogRecommendation, UnreliableReason,
    flatten_models_dev_catalog, recommend_model_catalog_preset, search_model_catalog,
};
pub use discovery::{DiscoveredModel, parse_discovered_models, build_models_list_url};
pub use provider_listing::{
    ProviderListingInput, ProviderAuthStatus, ApiKeyProviderListing, OAuthProviderListing,
    build_api_key_provider_list, build_oauth_provider_list,
};
