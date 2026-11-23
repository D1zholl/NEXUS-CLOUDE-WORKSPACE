//! Печатает OpenAPI-спецификацию в stdout.
//!
//! Используется для обновления `docs/openapi.json`:
//! `cargo run --example dump_openapi > docs/openapi.json`

fn main() {
    let spec = nexus::api::openapi::specification(env!("CARGO_PKG_VERSION"));
    match serde_json::to_string_pretty(&spec) {
        Ok(json) => println!("{json}"),
        Err(error) => {
            eprintln!("не удалось сериализовать OpenAPI: {error}");
            std::process::exit(1);
        }
    }
}
