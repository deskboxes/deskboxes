//! Setup's own strings, English and Polish (Windows' display language picks; `--lang en|pl`
//! overrides). Wording follows the app's: a box is "pole" in Polish, the settings window is
//! "Deskboxes settings" / "Ustawienia Deskboxes".

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En,
    Pl,
}

impl Lang {
    pub fn detect(arg: Option<&str>) -> Lang {
        match arg {
            Some("pl") => return Lang::Pl,
            Some("en") => return Lang::En,
            _ => {}
        }
        let id = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
        if id & 0x3ff == 0x15 {
            Lang::Pl
        } else {
            Lang::En
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum T {
    WindowInstall,
    WindowUninstall,
    Tagline,
    Version,
    VersionUpdate,
    Location,
    Change,
    PickFolder,
    Autostart,
    AutostartDesc,
    StartMenu,
    StartMenuDesc,
    Footnote,
    RunningNote,
    Cancel,
    Install,
    Update,
    Reinstall,
    Installing,
    Updating,
    StepClosing,
    StepCopying,
    StepRegistering,
    StepStarting,
    DoneTitle,
    DoneUpdated,
    DoneText,
    LaunchNow,
    LaunchNowDesc,
    Finish,
    Close,
    ErrorTitle,
    ErrorUninstallTitle,
    Details,
    TryAgain,
    ErrNoPayload,
    ErrRunning,
    ErrLocation,
    UninstallTitle,
    UninstallText,
    RemoveData,
    RemoveDataDesc,
    UninstallButton,
    Uninstalling,
    StepRestoring,
    StepRemoving,
    StepCleanup,
    UninstalledTitle,
    UninstalledText,
    DataKept,
    DataRemoved,
    NotInstalled,
    On,
    Off,
    ShortcutDesc,
    InstallTitle,
    UpdateTitle,
    WorkText,
    Percent,
    ReinstallTitle,
    VersionSame,
}

pub fn t(l: Lang, k: T) -> &'static str {
    use T::*;
    let (en, pl) = match k {
        WindowInstall => ("Deskboxes Setup", "Instalator Deskboxes"),
        WindowUninstall => ("Uninstall Deskboxes", "Odinstaluj Deskboxes"),
        Tagline => (
            "Boxes, tabs and quick search for your desktop. Boxes only arrange the icons: your files stay in the Desktop folder.",
            "Pola, karty i szybkie wyszukiwanie na pulpicie. Pola tylko porządkują ikony: pliki zostają w folderze Pulpit.",
        ),
        Version => ("Version {}", "Wersja {}"),
        VersionUpdate => ("Version {} · installed now: {}", "Wersja {} · teraz zainstalowana: {}"),
        Location => ("Install location", "Miejsce instalacji"),
        Change => ("Change", "Zmień"),
        PickFolder => ("Choose where to install Deskboxes", "Wybierz miejsce instalacji Deskboxes"),
        Autostart => ("Start with Windows", "Uruchamiaj z systemem Windows"),
        AutostartDesc => ("Deskboxes starts when you sign in", "Deskboxes uruchamia się po zalogowaniu"),
        StartMenu => ("Add to Start", "Dodaj do menu Start"),
        StartMenuDesc => ("A Deskboxes shortcut in the Start menu's list of apps", "Skrót Deskboxes na liście aplikacji w menu Start"),
        Footnote => ("For your account only · {} MB", "Tylko dla Twojego konta · {} MB"),
        RunningNote => ("Deskboxes is running: setup closes it and opens it again when it is done.", "Deskboxes działa: instalator zamknie go i otworzy ponownie po zakończeniu."),
        Cancel => ("Cancel", "Anuluj"),
        Install => ("Install", "Zainstaluj"),
        Update => ("Update", "Aktualizuj"),
        Reinstall => ("Reinstall", "Zainstaluj ponownie"),
        Installing => ("Installing Deskboxes", "Instalowanie Deskboxes"),
        Updating => ("Updating Deskboxes", "Aktualizowanie Deskboxes"),
        StepClosing => ("Closing Deskboxes", "Zamykanie Deskboxes"),
        StepCopying => ("Copying files ({} of {})", "Kopiowanie plików ({} z {})"),
        StepRegistering => ("Adding Deskboxes to Windows", "Dodawanie Deskboxes do systemu Windows"),
        StepStarting => ("Starting Deskboxes", "Uruchamianie Deskboxes"),
        DoneTitle => ("Deskboxes is installed", "Deskboxes jest zainstalowany"),
        DoneUpdated => ("Deskboxes is up to date", "Deskboxes jest zaktualizowany"),
        DoneText => (
            "To set it up your way, right-click the desktop and choose Deskboxes › Deskboxes settings.",
            "Aby dopasować go do siebie, kliknij pulpit prawym przyciskiem i wybierz Deskboxes › Ustawienia Deskboxes.",
        ),
        LaunchNow => ("Open Deskboxes now", "Otwórz Deskboxes teraz"),
        LaunchNowDesc => ("Your desktop switches to Deskboxes right away", "Pulpit od razu przełączy się na Deskboxes"),
        Finish => ("Finish", "Zakończ"),
        Close => ("Close", "Zamknij"),
        ErrorTitle => ("Setup couldn't finish", "Instalacja nie została ukończona"),
        ErrorUninstallTitle => ("Deskboxes couldn't be removed", "Nie udało się usunąć Deskboxes"),
        Details => ("Details: {}", "Szczegóły: {}"),
        TryAgain => ("Try again", "Spróbuj ponownie"),
        ErrNoPayload => (
            "This copy of the installer has no files to install. Build the full installer with build.ps1 ‑Installer.",
            "Ta kopia instalatora nie zawiera plików do zainstalowania. Pełny instalator tworzy build.ps1 ‑Installer.",
        ),
        ErrRunning => (
            "Deskboxes is still running and could not be closed. Close it (right-click the desktop › Deskboxes › Disable Deskboxes) and try again.",
            "Deskboxes nadal działa i nie udało się go zamknąć. Zamknij go (prawy przycisk na pulpicie › Deskboxes › Wyłącz Deskboxes) i spróbuj ponownie.",
        ),
        ErrLocation => ("Deskboxes can't be installed in this folder. Choose another location.", "Nie można zainstalować Deskboxes w tym folderze. Wybierz inne miejsce."),
        UninstallTitle => ("Uninstall Deskboxes", "Odinstaluj Deskboxes"),
        UninstallText => (
            "Your desktop goes back to the usual Windows icons. The files on it stay where they are.",
            "Pulpit wróci do zwykłych ikon Windows. Pliki na nim zostaną na swoim miejscu.",
        ),
        RemoveData => ("Also remove my boxes and settings", "Usuń też moje pola i ustawienia"),
        RemoveDataDesc => (
            "Layouts, saved layouts, backups and settings. Keep them to get your boxes back if you install Deskboxes again.",
            "Układy, zapisane układy, kopie zapasowe i ustawienia. Zachowaj je, aby odzyskać pola po ponownej instalacji.",
        ),
        UninstallButton => ("Uninstall", "Odinstaluj"),
        Uninstalling => ("Removing Deskboxes", "Usuwanie Deskboxes"),
        StepRestoring => ("Bringing back the desktop icons", "Przywracanie ikon pulpitu"),
        StepRemoving => ("Removing files", "Usuwanie plików"),
        StepCleanup => ("Removing Deskboxes from Windows", "Usuwanie Deskboxes z systemu Windows"),
        UninstalledTitle => ("Deskboxes was removed", "Deskboxes został usunięty"),
        UninstalledText => ("Your desktop shows the usual Windows icons again.", "Pulpit znów pokazuje zwykłe ikony Windows."),
        DataKept => ("Your boxes and settings were kept.", "Twoje pola i ustawienia zostały zachowane."),
        DataRemoved => ("Your boxes and settings were removed too.", "Twoje pola i ustawienia również zostały usunięte."),
        NotInstalled => ("Deskboxes is not installed.", "Deskboxes nie jest zainstalowany."),
        On => ("On", "Włączone"),
        Off => ("Off", "Wyłączone"),
        ShortcutDesc => ("Boxes, tabs and quick search for your desktop", "Pola, karty i szybkie wyszukiwanie na pulpicie"),
        InstallTitle => ("Install Deskboxes", "Zainstaluj Deskboxes"),
        UpdateTitle => ("Update Deskboxes", "Zaktualizuj Deskboxes"),
        WorkText => ("This takes a few seconds. Your desktop files are not touched.", "To potrwa kilka sekund. Pliki na pulpicie pozostają nietknięte."),
        Percent => ("{}%", "{}%"),
        ReinstallTitle => ("Reinstall Deskboxes", "Zainstaluj ponownie Deskboxes"),
        VersionSame => ("Version {} · already installed", "Wersja {} · już zainstalowana"),
    };
    match l {
        Lang::En => en,
        Lang::Pl => pl,
    }
}

/// `t` with each `{}` replaced by the next argument.
pub fn tf(l: Lang, k: T, args: &[&str]) -> String {
    let mut out = String::new();
    let mut it = args.iter();
    let mut rest = t(l, k);
    while let Some(i) = rest.find("{}") {
        out.push_str(&rest[..i]);
        out.push_str(it.next().copied().unwrap_or(""));
        rest = &rest[i + 2..];
    }
    out.push_str(rest);
    out
}
