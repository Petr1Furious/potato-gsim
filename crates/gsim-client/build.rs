//! Embeds the application icon and name in the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        res.set("ProductName", "Potato Gravity Simulator");
        res.set("FileDescription", "Potato Gravity Simulator");
        res.compile().expect("embed Windows resources");
    }
}
