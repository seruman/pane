//! Opt-in, same-process inspection. No second Script or Runtime is created.
//! Terminal captures reuse the host's retained offscreen image, never a second runtime.

#[cfg(unix)]
mod bridge;
#[cfg(unix)]
mod capture;
#[cfg(unix)]
mod commands;
#[cfg(unix)]
mod pixels;
#[cfg(unix)]
mod transport;
#[cfg(unix)]
mod tree;

use std::path::PathBuf;

/// Remove only our valued flag before the existing script/capability parser runs.
pub(crate) fn arguments(args: &[String]) -> Result<(Vec<String>, Option<PathBuf>), String> {
    let mut rest = Vec::new();
    let mut dir = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--inspect" {
            if dir.is_some() {
                return Err("--inspect may only be supplied once".into());
            }
            let value = args
                .next()
                .filter(|s| !s.is_empty() && !s.starts_with('-'))
                .ok_or("--inspect needs a private directory")?;
            dir = Some(PathBuf::from(value));
        } else {
            rest.push(arg.clone());
        }
    }
    Ok((rest, dir))
}

pub(crate) struct Guard {
    #[cfg(unix)]
    _server: transport::Server,
}

pub(crate) fn attach(
    app: rune::App<crate::State>,
    dir: Option<PathBuf>,
) -> Result<(rune::App<crate::State>, Option<Guard>), String> {
    let Some(dir) = dir else {
        return Ok((app, None));
    };
    if rune::is_headless() {
        return Err("--inspect is for live apps, not Rune's headless output flags".into());
    }
    #[cfg(unix)]
    {
        let (app, server) = bridge::attach(app, &dir)?;
        Ok((app, Some(Guard { _server: server })))
    }
    #[cfg(not(unix))]
    {
        let _ = (app, dir);
        Err("--inspect currently requires Unix domain sockets".into())
    }
}

#[cfg(test)]
mod tests;
