#![windows_subsystem = "windows"]
mod model;
mod ssh;
mod app;

use app::SshApp;

fn main() -> eframe::Result<()> {
    println!("Starting SSH File Browser...");
    let native_options = eframe::NativeOptions::default();
    let res = eframe::run_native(
        "SSH File Browser",
        native_options,
        Box::new(|cc| {
            println!("Creating app context...");
            Ok(Box::new(SshApp::new(cc)))
        }),
    );
    println!("Exiting with result: {:?}", res);
    res
}
