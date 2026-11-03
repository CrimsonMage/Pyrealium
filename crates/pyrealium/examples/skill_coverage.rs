//! Run after regenerating skill-effects.tsv to audit executable versus decoded records.
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = pyrealium::abilities::Runtime::load()?;
    println!("{}", serde_json::to_string_pretty(&runtime.coverage())?);
    Ok(())
}
