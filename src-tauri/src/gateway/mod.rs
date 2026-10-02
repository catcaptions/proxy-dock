pub mod router;

pub const PROVIDER_ORDER: [&str; 5] = ["commandcode", "opencode", "chatgpt", "antigravity", "claude"];

pub fn split_provider_model(model: &str) -> Option<(&str, &str)> {
    let (slug, native) = model.split_once('/')?;
    if slug.is_empty() || native.is_empty() {
        return None;
    }
    Some((slug, native))
}

// NOTE: the old `adapter_models` / `unified_models` placeholder fns were
// removed in 1A: the catalog now comes only from live provider fetches cached
// in `model_cache` (`crate::catalog`), never from `BUILTIN_MODELS`.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_native_id_after_first_slash() {
        assert_eq!(
            split_provider_model("commandcode/z-ai/glm-5.3-flash"),
            Some(("commandcode", "z-ai/glm-5.3-flash"))
        );
    }

    #[test]
    fn rejects_unqualified_ids() {
        assert_eq!(split_provider_model("gpt-5.4"), None);
    }
}
