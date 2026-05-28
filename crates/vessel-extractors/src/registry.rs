use std::sync::Arc;

use vessel_core::models::InputRef;

use crate::traits::{Extractor, SupportLevel};

#[derive(Default)]
pub struct ExtractorRegistry {
    extractors: Vec<Arc<dyn Extractor>>,
}

impl ExtractorRegistry {
    pub fn new(extractors: Vec<Arc<dyn Extractor>>) -> Self {
        Self { extractors }
    }

    pub fn register<E>(&mut self, extractor: E)
    where
        E: Extractor + 'static,
    {
        self.extractors.push(Arc::new(extractor));
    }

    pub fn register_arc(&mut self, extractor: Arc<dyn Extractor>) {
        self.extractors.push(extractor);
    }

    pub fn best_for(&self, input: &InputRef) -> Option<Arc<dyn Extractor>> {
        self.extractors
            .iter()
            .max_by_key(|extractor| extractor.supports(input))
            .filter(|extractor| extractor.supports(input) != SupportLevel::Unsupported)
            .cloned()
    }

    pub fn names(&self) -> Vec<String> {
        self.extractors
            .iter()
            .map(|extractor| extractor.name().to_owned())
            .collect()
    }
}
