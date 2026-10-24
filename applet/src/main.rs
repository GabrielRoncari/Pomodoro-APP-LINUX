//! foco-applet: miniaplicativo do Foco para o painel do COSMIC.
//! Mostra o anel e o tempo no painel; o clique abre os controles. Fala com o daemon `foco`.

mod app;
mod daemon;
mod ring;

fn main() -> cosmic::iced::Result {
    cosmic::applet::run::<app::Foco>(())
}
