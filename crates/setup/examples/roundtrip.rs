//! Real install → verify → autostart toggle → uninstall → verify-clean round trip on this machine.
//! The example exe itself plays "orbit.exe". Refuses to run if Orbit is already installed.
use orbit_setup::*;
use std::{path::Path, thread, time::Duration};
use windows::core::{w, Interface, HSTRING, PCWSTR};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ};
use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

const RUN: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const UNINST: PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Orbit");

fn reg_sz(key: PCWSTR, name: &str) -> Option<String> {
    let mut buf = [0u16; 1024];
    let mut len = 2048u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, key, &HSTRING::from(name), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len)) }
        .ok()
        .ok()?;
    Some(String::from_utf16_lossy(&buf[..len as usize / 2 - 1]))
}

fn reg_dword(key: PCWSTR, name: &str) -> Option<u32> {
    let mut v = 0u32;
    let mut len = 4u32;
    unsafe { RegGetValueW(HKEY_CURRENT_USER, key, &HSTRING::from(name), RRF_RT_REG_DWORD, None, Some((&mut v as *mut u32).cast()), Some(&mut len)) }
        .ok()
        .ok()?;
    Some(v)
}

fn main() {
    let exe = installed_exe();
    let dir = exe.parent().unwrap().to_path_buf();
    let lnk = Path::new(&std::env::var("APPDATA").unwrap()).join(r"Microsoft\Windows\Start Menu\Programs\Orbit.lnk");
    assert!(!dir.exists() && reg_sz(UNINST, "DisplayName").is_none(), "Orbit is already installed here; not touching it");
    assert!(!is_installed_copy());

    let got = install().expect("install");
    assert_eq!(got, exe);
    let ico = dir.join("orbit.ico");
    assert!(exe.metadata().unwrap().len() == std::env::current_exe().unwrap().metadata().unwrap().len());
    assert_eq!(&std::fs::read(&ico).unwrap()[..6], &[0, 0, 1, 0, 8, 0]);
    println!("files ok: {} + orbit.ico", exe.display());

    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
        link.cast::<IPersistFile>().unwrap().Load(&HSTRING::from(lnk.as_path()), STGM_READ).unwrap();
        let mut buf = [0u16; 260];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0).unwrap();
        let target = String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap()]);
        assert!(target.eq_ignore_ascii_case(&exe.to_string_lossy()), "lnk target {target}");
        let mut idx = 0;
        link.GetIconLocation(&mut buf, &mut idx).unwrap();
        let icon = String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap()]);
        assert!(icon.eq_ignore_ascii_case(&ico.to_string_lossy()), "lnk icon {icon}");
        link.GetDescription(&mut buf).unwrap();
        assert!(String::from_utf16_lossy(&buf).starts_with("The fastest way to act on files.\0"));
    }
    println!("shortcut ok: {}", lnk.display());

    let quoted = format!("\"{}\"", exe.display());
    assert_eq!(reg_sz(RUN, "Orbit").as_deref(), Some(quoted.as_str()));
    assert_eq!(reg_sz(UNINST, "DisplayName").as_deref(), Some("Orbit"));
    assert_eq!(reg_sz(UNINST, "DisplayVersion").as_deref(), Some(env!("CARGO_PKG_VERSION")));
    assert_eq!(reg_sz(UNINST, "Publisher").as_deref(), Some("Orbit"));
    assert_eq!(reg_sz(UNINST, "DisplayIcon"), Some(ico.to_string_lossy().into_owned()));
    assert_eq!(reg_sz(UNINST, "InstallLocation"), Some(dir.to_string_lossy().into_owned()));
    assert_eq!(reg_sz(UNINST, "UninstallString"), Some(format!("{quoted} --uninstall")));
    assert!(reg_dword(UNINST, "EstimatedSize").unwrap() > 0);
    assert_eq!((reg_dword(UNINST, "NoModify"), reg_dword(UNINST, "NoRepair")), (Some(1), Some(1)));
    println!("registry ok (EstimatedSize {} KB)", reg_dword(UNINST, "EstimatedSize").unwrap());

    assert!(autostart());
    set_autostart(false).unwrap();
    assert!(!autostart() && reg_sz(RUN, "Orbit").is_none());
    set_autostart(true).unwrap();
    assert!(autostart());
    println!("autostart toggle ok");

    let again = install().expect("re-install (upgrade in place)");
    assert_eq!(again, exe);
    println!("idempotent re-install ok");

    uninstall().expect("uninstall");
    assert!(!lnk.exists() && reg_sz(RUN, "Orbit").is_none() && reg_sz(UNINST, "DisplayName").is_none());
    for _ in 0..100 {
        if !dir.exists() {
            println!("uninstall ok: shortcut, Run value, uninstall key and {} all gone", dir.display());
            return;
        }
        thread::sleep(Duration::from_millis(100));
    }
    panic!("{} still exists after 10 s", dir.display());
}
