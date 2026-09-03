fn main() {
    let config = slint_build::CompilerConfiguration::new()
        .with_style("fluent-dark".into())
        .with_bundled_translations("../../lang")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    println!("cargo::rerun-if-changed=../../lang");
    slint_build::compile_with_config("ui/app-window.slint", config)
        .expect("the user interface compiles");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        println!("cargo::rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    }
}
