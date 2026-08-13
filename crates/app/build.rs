//! Embed Windows resources into the exe: an icon, version info, and an
//! application manifest (DPI awareness + Windows 10/11 compatibility). This
//! makes the exe look properly published (real publisher/description fields
//! instead of "unknown"); it does NOT replace code signing.

fn main() {
    #[cfg(windows)]
    embed_windows_resources();
}

#[cfg(windows)]
fn embed_windows_resources() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let icon = format!("{manifest_dir}/../../assets/icon.ico");
    println!("cargo:rerun-if-changed={icon}");
    println!("cargo:rerun-if-changed=build.rs");

    const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="rUEXDataRunner" version="0.1.0.0"/>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">permonitorv2,permonitor</dpiAwareness>
      <activeCodePage xmlns="http://schemas.microsoft.com/SMI/2019/WindowsSettings">UTF-8</activeCodePage>
    </windowsSettings>
  </application>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
</assembly>"#;

    let mut res = winresource::WindowsResource::new();
    res.set_icon(&icon);
    res.set("ProductName", "rUEXDataRunner");
    res.set("FileDescription", "Star Citizen to UEX datarunner");
    res.set("CompanyName", "rUEXDataRunner");
    res.set("LegalCopyright", "MIT licensed. Not affiliated with UEX or RSI.");
    res.set("OriginalFilename", "ruex-datarunner.exe");
    res.set("InternalName", "ruex-datarunner");
    res.set("FileVersion", "0.1.0.0");
    res.set("ProductVersion", "0.1.0.0");
    res.set_manifest(MANIFEST);

    if let Err(e) = res.compile() {
        // Don't fail the build if the resource compiler isn't available; the exe
        // just won't carry the icon/version metadata.
        println!("cargo:warning=winresource: could not embed resources: {e}");
    }
}
