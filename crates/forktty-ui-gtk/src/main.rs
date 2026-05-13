#[cfg(feature = "gtk-vte")]
mod gtk_app;

#[cfg(feature = "gtk-vte")]
fn main() {
    gtk_app::run();
}

#[cfg(not(feature = "gtk-vte"))]
fn main() {
    println!(
        "forktty-ui-gtk built without GTK/VTE. Rebuild with `--features gtk-vte` after installing GTK4, libadwaita, and vte-2.91-gtk4 development packages."
    );
}
