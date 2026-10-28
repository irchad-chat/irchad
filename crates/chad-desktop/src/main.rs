use tracing_subscriber::EnvFilter;

fn main() -> eframe::Result {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("IrChad")
            .with_app_id("party.gentoo.IrChad")
            .with_inner_size([1100.0, 720.0]),
        ..Default::default()
    };
    eframe::run_native("IrChad", options, Box::new(|_cc| Ok(Box::new(App))))
}

struct App;

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("IrChad");
        });
    }
}
