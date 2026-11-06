//! Onglets qui jouent du son, lus avec UI Automation.
//!
//! Chromium (Brave, Chrome, Edge) ajoute au nom de l'onglet une mention comme « Lecture audio ».
//! On lit seulement la barre d'onglets du navigateur, jamais le contenu des pages : sinon les
//! filtres d'une page YouTube, par exemple, ressemblent à des onglets.
//! Le travail se fait sur un thread MTA sans fenêtre, qui se termine après chaque passage.
//! Les titres restent en mémoire le temps de l'affichage et ne sont jamais journalisés.

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::System::Com::*;
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Accessibility::*;
use windows::core::Result;

use crate::model::*;

const BROWSERS: [&str; 6] = ["chrome.exe", "msedge.exe", "brave.exe", "firefox.exe", "opera.exe", "vivaldi.exe"];

/// Mentions relevées sur Brave, Chrome et Edge, en minuscules. Une liste exacte évite de
/// prendre pour un onglet sonore une page dont le titre finit simplement par « audio ».
const AUDIBLE_LABELS: [&str; 17] = [
    "lecture audio",
    "audio playing",
    "audiowiedergabe",
    "audio wird wiedergegeben",
    "reproducción de audio",
    "riproduzione audio in corso",
    "riproduzione audio",
    "reprodução de áudio",
    "áudio em reprodução",
    "audio wordt afgespeeld",
    "odtwarzanie dźwięku",
    "воспроизводится аудио",
    "звук воспроизводится",
    "オーディオ再生中です",
    "オーディオを再生中",
    "正在播放音频",
    "音频正在播放",
];
/// Pendant un appel, le navigateur remplace « Lecture audio » par l'une de ces mentions :
/// un onglet Teams qui utilise le micro n'annonce donc pas son son. Relevées dans les fichiers
/// de langue de Brave, Chrome et Edge.
const MICROPHONE_LABELS: [&str; 30] = [
    "enregistrement du micro",
    "enregistrement de la caméra et du micro",
    "enregistrement par microphone",
    "enregistrement de la caméra et du microphone",
    "microphone recording",
    "camera and microphone recording",
    "mikrofon zeichnet auf",
    "kamera und mikrofon zeichnen auf",
    "mikrofonaufzeichnung",
    "kamera- und mikrofonaufzeichnung",
    "grabación del micrófono",
    "grabación de la cámara y el micrófono",
    "grabación de cámara y micrófono",
    "registrazione con microfono",
    "registrazione con videocamera e microfono",
    "registrazione microfono",
    "registrazione della webcam o del microfono",
    "gravação de microfone",
    "gravação de câmera e microfone",
    "microfoonopname",
    "camera- en microfoonopname",
    "nagrywanie za pomocą mikrofonu",
    "nagrywanie za pomocą kamery i mikrofonu",
    "nagrywanie z mikrofonu",
    "nagrywanie z kamery i mikrofonu",
    "запись с микрофона",
    "запись с камеры и микрофона",
    "マイク録音",
    "麦克风正在录音",
    "麦克风录制",
];
const MAX_DEPTH: u32 = 14;

/// Un seul passage à la fois : un navigateur figé ne doit pas accumuler les threads.
static SCAN_RUNNING: AtomicBool = AtomicBool::new(false);

pub fn is_browser(path: &str) -> bool {
    let file = path.rsplit('\\').next().unwrap_or(path).to_ascii_lowercase();
    BROWSERS.contains(&file.as_str())
}

/// Renvoie le titre sans sa mention, et si l'onglet utilise le micro, s'il est concerné par le son.
/// La mention n'est pas toujours à la fin : Brave peut ajouter ensuite la mémoire utilisée
/// (« Titre – Lecture audio - Utilisation de la mémoire - 19 Mo »). On teste donc chaque segment,
/// en partant de la fin, pour qu'un titre contenant lui-même un séparateur reste entier.
/// Les espaces peuvent être insécables : c'est le cas du français dans Brave.
pub fn tab_status(name: &str) -> Option<(&str, bool)> {
    let boundaries = separator_positions(name);
    for (position, (index, length)) in boundaries.iter().enumerate().rev() {
        let start = index + length;
        let end = boundaries.get(position + 1).map_or(name.len(), |(next, _)| *next);
        let label = name[start..end].trim().replace('\u{a0}', " ").to_lowercase();
        let uses_microphone = MICROPHONE_LABELS.contains(&label.as_str());
        if uses_microphone || AUDIBLE_LABELS.contains(&label.as_str()) {
            return Some((name[..*index].trim_end(), uses_microphone));
        }
    }
    None
}

/// Positions et longueurs des séparateurs : un tiret entouré d'espaces, ou « : » suivi d'une espace.
fn separator_positions(name: &str) -> Vec<(usize, usize)> {
    let mut positions = Vec::new();
    let mut previous = None;
    let mut characters = name.char_indices().peekable();
    while let Some((index, character)) = characters.next() {
        let next_is_space = characters.peek().is_some_and(|(_, c)| c.is_whitespace());
        let previous_is_space = previous.is_some_and(char::is_whitespace);
        let is_separator = next_is_space
            && match character {
                '-' | '–' | '—' => previous_is_space,
                ':' => true,
                _ => false,
            };
        if is_separator {
            positions.push((index, character.len_utf8()));
        }
        previous = Some(character);
    }
    positions
}

fn with_uia<T>(work: impl FnOnce(&IUIAutomation) -> T) -> Option<T> {
    unsafe {
        if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
            return None;
        }
        let result = CoCreateInstance::<_, IUIAutomation>(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            .ok()
            .map(|automation| work(&automation));
        CoUninitialize();
        result
    }
}

/// Cherche la barre d'onglets en profondeur, sans jamais entrer dans un document web.
fn find_tab_strip(
    automation: &IUIAutomation,
    element: &IUIAutomationElement,
    tab_item: &IUIAutomationCondition,
    any: &IUIAutomationCondition,
    depth: u32,
) -> Option<IUIAutomationElementArray> {
    if depth > MAX_DEPTH {
        return None;
    }
    unsafe {
        let children = element.FindAll(TreeScope_Children, any).ok()?;
        for i in 0..children.Length().ok()? {
            let Ok(child) = children.GetElement(i) else { continue };
            let Ok(kind) = child.CurrentControlType() else { continue };
            if kind == UIA_DocumentControlTypeId {
                continue;
            }
            if kind == UIA_TabControlTypeId {
                if let Ok(items) = child.FindAll(TreeScope_Children, tab_item) {
                    if items.Length().unwrap_or(0) > 0 {
                        return Some(items);
                    }
                }
            }
            if let Some(found) = find_tab_strip(automation, &child, tab_item, any, depth + 1) {
                return Some(found);
            }
        }
    }
    None
}

fn tab_strip(automation: &IUIAutomation, hwnd: isize) -> Result<Option<IUIAutomationElementArray>> {
    unsafe {
        let root = automation.ElementFromHandle(to_hwnd(hwnd))?;
        let tab_item = automation
            .CreatePropertyCondition(UIA_ControlTypePropertyId, &VARIANT::from(UIA_TabItemControlTypeId.0 as i32))?;
        let any = automation.CreateTrueCondition()?;
        Ok(find_tab_strip(automation, &root, &tab_item, &any, 0))
    }
}

/// Lit les onglets sonores des fenêtres indiquées, puis poste le résultat à l'interface.
pub fn scan_async(ui: isize, jobs: Vec<(u32, isize)>) {
    if SCAN_RUNNING.swap(true, Ordering::AcqRel) {
        return;
    }
    std::thread::spawn(move || {
        let results = with_uia(|automation| {
            let mut results: Vec<AppTabs> = Vec::new();
            for (app, hwnd) in &jobs {
                let Ok(Some(items)) = tab_strip(automation, *hwnd) else { continue };
                for i in 0..unsafe { items.Length() }.unwrap_or(0) {
                    let Ok(name) = (unsafe { items.GetElement(i).and_then(|e| e.CurrentName()) }) else { continue };
                    let name = name.to_string();
                    let Some((title, uses_mic)) = tab_status(&name) else { continue };
                    let occurrence = results.iter()
                        .filter(|r| r.app == *app)
                        .flat_map(|r| r.tabs.iter())
                        .filter(|t| t.hwnd == *hwnd && t.title == title)
                        .count();
                    let tab = TabItem { hwnd: *hwnd, title: title.to_string(), occurrence, uses_mic, extension: None };
                    match results.iter_mut().find(|r| r.app == *app) {
                        Some(entry) => entry.tabs.push(tab),
                        None => results.push(AppTabs { app: *app, tabs: vec![tab] }),
                    }
                }
            }
            results
        });
        SCAN_RUNNING.store(false, Ordering::Release);
        if let Some(results) = results {
            post_ui_box(ui, WM_U_TABS, Box::new(results));
        }
    });
}

/// Sélectionne un onglet d'après son titre et son rang, car l'ordre a pu changer depuis la lecture.
pub fn select_tab_async(hwnd: isize, title: String, occurrence: usize) {
    std::thread::spawn(move || {
        with_uia(|automation| -> Result<()> {
            let Some(items) = tab_strip(automation, hwnd)? else { return Ok(()) };
            let mut seen = 0;
            unsafe {
                for i in 0..items.Length()? {
                    let element = items.GetElement(i)?;
                    let name = element.CurrentName()?.to_string();
                    if tab_status(&name).is_some_and(|(found, _)| found == title) {
                        if seen == occurrence {
                            let pattern: IUIAutomationSelectionItemPattern =
                                element.GetCurrentPatternAs(UIA_SelectionItemPatternId)?;
                            return pattern.Select();
                        }
                        seen += 1;
                    }
                }
            }
            Ok(())
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audible_title(name: &str) -> Option<&str> {
        tab_status(name).map(|(title, _)| title)
    }

    #[test]
    fn reconnait_un_appel_teams_par_son_micro() {
        let name = "Équipes et canaux | Réunion\u{a0}–\u{a0}Enregistrement du micro";
        assert_eq!(tab_status(name), Some(("Équipes et canaux | Réunion", true)));
        let with_memory = "Réunion\u{a0}–\u{a0}Enregistrement du micro - Utilisation de la mémoire - 1,0\u{a0}Go";
        assert_eq!(tab_status(with_memory), Some(("Réunion", true)));
        assert_eq!(tab_status("Appel - Microphone recording"), Some(("Appel", true)));
    }

    #[test]
    fn la_lecture_audio_nutilise_pas_le_micro() {
        assert_eq!(tab_status("Vidéo – Lecture audio"), Some(("Vidéo", false)));
        assert_eq!(tab_status("Vidéo – Son coupé"), None);
    }

    #[test]
    fn accepte_les_espaces_insecables_du_francais() {
        let name = "MelangeurTestSon\u{a0}–\u{a0}Lecture audio - Utilisation de la mémoire - 19,7\u{a0}Mo";
        assert_eq!(audible_title(name), Some("MelangeurTestSon"));
    }

    #[test]
    fn reconnait_les_mentions_relevees() {
        assert_eq!(audible_title("Ma vidéo – Lecture audio"), Some("Ma vidéo"));
        assert_eq!(audible_title("Ma vidéo - Lecture audio"), Some("Ma vidéo"));
        assert_eq!(audible_title("Song - Audio playing"), Some("Song"));
        assert_eq!(audible_title("Lied – Audiowiedergabe"), Some("Lied"));
        assert_eq!(audible_title("Canción: reproducción de audio"), Some("Canción"));
        assert_eq!(audible_title("歌 - 正在播放音频"), Some("歌"));
    }

    #[test]
    fn accepte_la_memoire_ajoutee_par_brave() {
        assert_eq!(
            audible_title("Ma vidéo – Lecture audio - Utilisation de la mémoire - 19,2 Mo"),
            Some("Ma vidéo")
        );
        assert_eq!(audible_title("Ma page - Utilisation de la mémoire - 19,2 Mo"), None);
    }

    #[test]
    fn garde_le_dernier_separateur() {
        assert_eq!(audible_title("A - B - Lecture audio"), Some("A - B"));
    }

    #[test]
    fn ignore_les_autres_titres() {
        assert_eq!(audible_title("Tout"), None);
        assert_eq!(audible_title("Source : Aywen"), None);
        assert_eq!(audible_title("Guide - audio"), None);
        assert_eq!(audible_title("Lecture audio"), None);
    }
}
