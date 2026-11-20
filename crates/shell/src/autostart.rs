//! Starting Deskboxes at sign-in. Preferred: a Task Scheduler logon task for this user (starts as
//! soon as the user signs in, above-normal priority, normal rights; the HKCU Run entry Windows
//! starts late and at low priority, which made the desktop come up slowly on a slower PC).
//! Creating a logon task for one's own account needs no admin. If the task cannot be made, the Run
//! entry is used as before. Shared by the installer and the core's "Start with Windows" toggle.

use crate::reg;
use std::path::Path;
use windows::core::BSTR;
use windows::Win32::System::Com::*;
use windows::Win32::System::TaskScheduler::*;
use windows::Win32::System::Variant::VARIANT;

/// The task's name (in the root folder) and the Run entry's value name.
pub const NAME: &str = "Deskboxes";

fn folder() -> windows::core::Result<ITaskFolder> {
    unsafe {
        let ts: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
        let v = VARIANT::default();
        ts.Connect(&v, &v, &v, &v)?;
        ts.GetFolder(&BSTR::from("\\"))
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn user() -> String {
    let name = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => format!("{d}\\{name}"),
        _ => name,
    }
}

/// The task: at this user's logon, no delay, priority 3 (above normal), no time limit, not
/// stopped on battery, never a second copy.
fn task_xml(watchdog: &Path) -> String {
    let u = xml_escape(&user());
    let exe = xml_escape(&watchdog.display().to_string());
    let dir = xml_escape(&watchdog.parent().map(|p| p.display().to_string()).unwrap_or_default());
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Starts Deskboxes when you sign in.</Description></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{u}</UserId></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><UserId>{u}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>false</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>3</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>"{exe}"</Command><Arguments>--launch</Arguments><WorkingDirectory>{dir}</WorkingDirectory></Exec></Actions>
</Task>"#
    )
}

fn create_task(watchdog: &Path) -> windows::core::Result<()> {
    unsafe {
        let f = folder()?;
        let v = VARIANT::default();
        f.RegisterTask(&BSTR::from(NAME), &BSTR::from(task_xml(watchdog)), TASK_CREATE_OR_UPDATE.0, &v, &v, TASK_LOGON_INTERACTIVE_TOKEN, &v)?;
    }
    Ok(())
}

fn task() -> Option<IRegisteredTask> {
    unsafe { folder().ok()?.GetTask(&BSTR::from(NAME)).ok() }
}

/// The task's program, if the task exists and is enabled.
fn task_target() -> Option<String> {
    let t = task()?;
    unsafe {
        if !t.Enabled().ok()?.as_bool() {
            return None;
        }
        let xml = t.Xml().ok()?.to_string();
        let a = xml.find("<Command>")? + "<Command>".len();
        let b = xml[a..].find("</Command>")? + a;
        Some(xml[a..b].replace("&quot;", "").replace("&amp;", "&").trim_matches('"').to_string())
    }
}

fn delete_task() {
    unsafe {
        if let Ok(f) = folder() {
            let _ = f.DeleteTask(&BSTR::from(NAME), 0);
        }
    }
}

fn run_approved() -> bool {
    reg::get_binary(reg::RUN_APPROVED, NAME).map(|b| b.first().map(|x| x & 1 == 0).unwrap_or(true)).unwrap_or(true)
}

/// Start the watchdog at sign-in: the logon task, else the Run entry. Returns which one.
pub fn enable(watchdog: &Path) -> windows::core::Result<&'static str> {
    match create_task(watchdog) {
        Ok(()) => {
            reg::delete_value(reg::RUN, NAME);
            reg::delete_value(reg::RUN_APPROVED, NAME);
            Ok("task")
        }
        Err(e) => {
            log::warn!("autostart: logon task failed ({e}); using the Run entry");
            reg::set_string(reg::RUN, NAME, &format!("\"{}\" --launch", watchdog.display()))?;
            reg::delete_value(reg::RUN_APPROVED, NAME);
            Ok("run")
        }
    }
}

pub fn disable() {
    delete_task();
    reg::delete_value(reg::RUN, NAME);
    reg::delete_value(reg::RUN_APPROVED, NAME);
}

/// The program sign-in will start, if any (task first, then an approved Run entry).
pub fn target() -> Option<String> {
    if let Some(t) = task_target() {
        return Some(t);
    }
    let run = reg::get_string(reg::RUN, NAME)?;
    run_approved().then(|| run.trim_start_matches('"').split('"').next().unwrap_or("").to_string())
}

pub fn is_on() -> bool {
    target().is_some()
}

/// Will sign-in start Deskboxes (the program it names exists)?
pub fn intact() -> bool {
    target().map(|t| Path::new(&t).exists()).unwrap_or(false)
}

/// Does the autostart point into `dir` (an install folder)?
pub fn points_into(dir: &Path) -> bool {
    target().map(|t| Path::new(&t).starts_with(dir)).unwrap_or(false)
}
