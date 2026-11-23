// Incrusta el ícono y los datos del creador en el .exe de Windows
// (se ven en el Explorador → clic derecho → Propiedades → Detalles).
fn main() {
    println!("cargo:rerun-if-changed=assets/app.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/app.ico")
            .set("ProductName", "Tokenscopio")
            .set("FileDescription", "Tokenscopio — uso de tus agentes de IA")
            .set("CompanyName", "Julio Carrizales")
            .set("LegalCopyright", "© 2026 Julio Carrizales");
        res.compile().expect("no se pudo incrustar el ícono");
    }
}
