use std::{io::stdout, panic};

use color_eyre::{config::HookBuilder, eyre};
use crossterm::{
    event::DisableMouseCapture,
    execute,
    terminal::{disable_raw_mode, LeaveAlternateScreen},
};

/**
This replaces the standard color_eyre panic and error hooks with hooks that
restore the terminal before printing the panic or error.
*/
pub fn install_hooks() -> color_eyre::Result<()> {
    let (panic_hook, eyre_hook) = HookBuilder::default().into_hooks();

    // convert from a color_eyre PanicHook to a standard panic hook
    let panic_hook = panic_hook.into_panic_hook();
    panic::set_hook(Box::new(move |panic_info| {
        restore().unwrap();
        panic_hook(panic_info);
    }));

    // convert from a color_eyre EyreHook to a eyre ErrorHook
    let eyre_hook = eyre_hook.into_eyre_hook();
    eyre::set_hook(Box::new(
        move |error: &(dyn std::error::Error + 'static)| {
            restore().unwrap();
            eyre_hook(error)
        },
    ))?;

    Ok(())
}

pub fn restore() -> color_eyre::Result<()> {
    let _ = disable_raw_mode();
    execute!(stdout(), LeaveAlternateScreen, DisableMouseCapture)?;
    Ok(())
}
