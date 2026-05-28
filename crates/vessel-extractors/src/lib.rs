pub mod plugins;
pub mod registry;
pub mod traits;
pub mod youtube;

pub use plugins::{LoadedPlugin, PluginCatalog, PluginLoadError, ResolvedProvider, load_plugins};
pub use registry::ExtractorRegistry;
pub use traits::{ExtractContext, ExtractRequest, ExtractedItem, Extractor, SupportLevel};
