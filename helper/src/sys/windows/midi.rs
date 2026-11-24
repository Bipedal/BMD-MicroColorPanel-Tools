//! MIDI ports through WinMM (Windows MIDI Services loopbacks, loopMIDI, hardware).

use std::sync::Mutex;
use windows_sys::Win32::Media::Audio::{
    midiInClose, midiInGetDevCapsW, midiInGetNumDevs, midiInOpen, midiInStart, midiInStop, midiOutClose,
    midiOutGetDevCapsW, midiOutGetNumDevs, midiOutOpen, midiOutShortMsg, CALLBACK_FUNCTION, CALLBACK_NULL,
    HMIDIIN, HMIDIOUT, MIDIINCAPSW, MIDIOUTCAPSW,
};

const MIM_DATA: u32 = 0x3C3;

struct Handles { out: HMIDIOUT, inp: HMIDIIN }
unsafe impl Send for Handles {}
static HANDLES: Mutex<Handles> = Mutex::new(Handles { out: std::ptr::null_mut(), inp: std::ptr::null_mut() });

fn wname(a: &[u16]) -> String {
    let n = a.iter().position(|c| *c == 0).unwrap_or(a.len());
    String::from_utf16_lossy(&a[..n])
}

pub fn out_names() -> Vec<String> {
    (0..unsafe { midiOutGetNumDevs() }).map(|i| {
        let mut c: MIDIOUTCAPSW = unsafe { std::mem::zeroed() };
        unsafe { midiOutGetDevCapsW(i as usize, &mut c, std::mem::size_of::<MIDIOUTCAPSW>() as u32) };
        let name = c.szPname;
        wname(&name)
    }).collect()
}
pub fn in_names() -> Vec<String> {
    (0..unsafe { midiInGetNumDevs() }).map(|i| {
        let mut c: MIDIINCAPSW = unsafe { std::mem::zeroed() };
        unsafe { midiInGetDevCapsW(i as usize, &mut c, std::mem::size_of::<MIDIINCAPSW>() as u32) };
        let name = c.szPname;
        wname(&name)
    }).collect()
}

unsafe extern "system" fn in_cb(_h: HMIDIIN, msg: u32, _inst: usize, p1: usize, _p2: usize) {
    if msg != MIM_DATA { return; }
    crate::midi::received((p1 & 0xFF) as u8, ((p1 >> 8) & 0x7F) as u8, ((p1 >> 16) & 0x7F) as u8);
}

/// Close the open port and open this one (None = just close).
pub fn open_out(i: Option<usize>) -> Result<(), String> {
    let mut h = HANDLES.lock().unwrap();
    if !h.out.is_null() { unsafe { midiOutClose(h.out) }; h.out = std::ptr::null_mut(); }
    let Some(i) = i else { return Ok(()) };
    let mut o: HMIDIOUT = std::ptr::null_mut();
    let r = unsafe { midiOutOpen(&mut o, i as u32, 0, 0, CALLBACK_NULL) };
    if r != 0 { return Err(format!("error {r}")); }
    h.out = o;
    Ok(())
}
pub fn open_in(i: Option<usize>) -> Result<(), String> {
    let mut h = HANDLES.lock().unwrap();
    if !h.inp.is_null() { unsafe { midiInStop(h.inp); midiInClose(h.inp) }; h.inp = std::ptr::null_mut(); }
    let Some(i) = i else { return Ok(()) };
    let mut n: HMIDIIN = std::ptr::null_mut();
    let r = unsafe { midiInOpen(&mut n, i as u32, in_cb as *const () as usize, 0, CALLBACK_FUNCTION) };
    if r != 0 { return Err(format!("error {r}")); }
    unsafe { midiInStart(n) };
    h.inp = n;
    Ok(())
}

/// One channel message; false if no port is open.
pub fn send(m: [u8; 3]) -> bool {
    let h = HANDLES.lock().unwrap();
    if h.out.is_null() { return false; }
    let w = m[0] as u32 | (m[1] as u32) << 8 | (m[2] as u32) << 16;
    unsafe { midiOutShortMsg(h.out, w) == 0 }
}

static LOOP_TRIED: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// Windows MIDI Services loopbacks don't survive a restart of the MIDI service in the current
/// preview (--save-to-config doesn't save), so with midi_out = "auto" and no loopback port the
/// helper creates the standard pair itself, at most every 5 minutes.
pub fn make_loopback() {
    {
        let mut g = LOOP_TRIED.lock().unwrap();
        if g.map_or(false, |t| t.elapsed() < std::time::Duration::from_secs(300)) { return; }
        *g = Some(std::time::Instant::now());
    }
    let exe = std::path::Path::new(r"C:\Program Files\Windows MIDI Services\Tools\midi.exe");
    if !exe.exists() { return; }
    use std::os::windows::process::CommandExt;
    let r = std::process::Command::new(exe)
        .args(["loopback", "create", "--root-name", "Default App Loopback", "--unique-identifier", "DEFAULT"])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output();
    match r {
        Ok(o) if o.status.success() => crate::util::log("midi: created the Default App Loopback (A)/(B) pair"),
        Ok(o) => crate::util::log(&format!("midi: creating the loopback failed: {}", String::from_utf8_lossy(&o.stdout).lines().last().unwrap_or(""))),
        Err(e) => crate::util::log(&format!("midi: can't run midi.exe: {e}")),
    }
}
