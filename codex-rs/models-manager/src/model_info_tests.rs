use super::*;
use crate::ModelsManagerConfig;
use pretty_assertions::assert_eq;

#[test]
fn reasoning_summaries_override_true_enables_support() {
    let model = model_info_from_slug("unknown-model");
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(true),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);
    let mut expected = model;
    expected.supports_reasoning_summaries = true;

    assert_eq!(updated, expected);
}

#[test]
fn reasoning_summaries_override_false_does_not_disable_support() {
    let mut model = model_info_from_slug("unknown-model");
    model.supports_reasoning_summaries = true;
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(false),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn reasoning_summaries_override_false_is_noop_when_model_is_false() {
    let model = model_info_from_slug("unknown-model");
    let config = ModelsManagerConfig {
        model_supports_reasoning_summaries: Some(false),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn deepseek_builtin_models_default_to_no_thinking() {
    let models = builtin_provider_models("DeepSeek", Some("https://api.deepseek.com/v1"));
    let slugs = models
        .iter()
        .map(|model| (model.slug.as_str(), model.default_reasoning_level))
        .collect::<Vec<_>>();

    assert_eq!(
        slugs,
        vec![
            ("deepseek-v4-pro", Some(ReasoningEffort::None)),
            ("deepseek-v4-flash", Some(ReasoningEffort::None)),
        ]
    );
    assert!(
        models
            .iter()
            .all(|model| model.base_instructions.contains("DeepSeek V4"))
    );
}
