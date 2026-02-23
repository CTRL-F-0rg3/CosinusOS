// src/userspace/src/terminal.rs — CosinusOS Terminal v1.0
//
// Pełny terminal tekstowy działający w userspace (no_std).
//
// Obsługiwane komendy:
//   ls   [ścieżka]         — listuj katalog
//   cd   <ścieżka>         — zmień katalog roboczy
//   pwd                    — pokaż bieżący katalog
//   cat  <plik>            — wyświetl zawartość pliku
//   touch <plik>           — utwórz pusty plik
//   mkdir <katalog>        — utwórz katalog
//   rm   <plik>            — usuń plik
//   rmdir <katalog>        — usuń katalog
//   echo <tekst>           — wypisz tekst
//   msg  <tekst>           — wyświetl wiadomość w ramce
//   write <plik> <tekst>   — zapisz tekst do pliku (nadpisuje)
//   append <plik> <tekst>  — dołącz tekst do pliku
//   stat <plik>            — informacje o pliku/katalogu
//   clear                  — wyczyść ekran terminalowy
//   help                   — lista komend
//   exit                   — zakończ terminal

extern crate alloc;

use alloc::vec::Vec;
use alloc::string::String;
use alloc::string::ToString;
use alloc::format;

use crate::files::{
    FsResult, FsError, FileStat, FileHandle, OpenFlags,
    DiskId, PartId, FsPath,
    parse_path, path_join, path_dirname, path_basename, path_is_safe,
    vfs_init, vfs_mount, file_read_all, file_write_all, file_stat,
    dir_list, dir_create, file_remove, VFS,
    RamFs,
};
use crate::{print, println, print_fmt, println_fmt, SpinLock};

// ============================================================================
// KOLORY ANSI (przez print do VGA / serial)
// Używamy sekwencji ANSI — o ile terminal nadrzędny je obsługuje.
// W środowisku CosinusOS traktujemy je jako metadane; VGA driver może je zinterpretować.
// ============================================================================

pub mod color {
    pub const RESET:   &str = "\x1b[0m";
    pub const BOLD:    &str = "\x1b[1m";
    pub const DIM:     &str = "\x1b[2m";

    // Foreground
    pub const BLACK:   &str = "\x1b[30m";
    pub const RED:     &str = "\x1b[31m";
    pub const GREEN:   &str = "\x1b[32m";
    pub const YELLOW:  &str = "\x1b[33m";
    pub const BLUE:    &str = "\x1b[34m";
    pub const MAGENTA: &str = "\x1b[35m";
    pub const CYAN:    &str = "\x1b[36m";
    pub const WHITE:   &str = "\x1b[37m";

    // Bright foreground
    pub const BR_BLACK:   &str = "\x1b[90m";
    pub const BR_RED:     &str = "\x1b[91m";
    pub const BR_GREEN:   &str = "\x1b[92m";
    pub const BR_YELLOW:  &str = "\x1b[93m";
    pub const BR_BLUE:    &str = "\x1b[94m";
    pub const BR_MAGENTA: &str = "\x1b[95m";
    pub const BR_CYAN:    &str = "\x1b[96m";
    pub const BR_WHITE:   &str = "\x1b[97m";

    // Background
    pub const BG_BLACK:  &str = "\x1b[40m";
    pub const BG_BLUE:   &str = "\x1b[44m";
    pub const BG_GREEN:  &str = "\x1b[42m";
    pub const BG_RED:    &str = "\x1b[41m";
    pub const BG_YELLOW: &str = "\x1b[43m";
    pub const BG_WHITE:  &str = "\x1b[47m";
    pub const BG_CYAN:   &str = "\x1b[46m";
}

use color::*;

// ============================================================================
// HISTORIA KOMEND
// ============================================================================

const HISTORY_MAX: usize = 64;

pub struct History {
    entries: Vec<String>,
    pos:     usize,   // indeks nawigacji (history_prev/next)
}

impl History {
    pub fn new() -> Self {
        Self { entries: Vec::new(), pos: 0 }
    }

    pub fn push(&mut self, cmd: String) {
        if cmd.is_empty() { return; }
        // Nie duplikuj ostatniego wpisu
        if self.entries.last().map(|s| s.as_str()) == Some(cmd.as_str()) {
            self.pos = self.entries.len();
            return;
        }
        if self.entries.len() >= HISTORY_MAX {
            self.entries.remove(0);
        }
        self.entries.push(cmd);
        self.pos = self.entries.len();
    }

    /// Poprzednia komenda (strzałka góra).
    pub fn prev(&mut self) -> Option<&str> {
        if self.entries.is_empty() { return None; }
        if self.pos > 0 { self.pos -= 1; }
        self.entries.get(self.pos).map(|s| s.as_str())
    }

    /// Następna komenda (strzałka dół).
    pub fn next(&mut self) -> Option<&str> {
        if self.pos < self.entries.len() { self.pos += 1; }
        self.entries.get(self.pos).map(|s| s.as_str())
    }

    pub fn len(&self) -> usize { self.entries.len() }
    pub fn get(&self, i: usize) -> Option<&str> { self.entries.get(i).map(|s| s.as_str()) }
}

// ============================================================================
// BUFOR EDYCJI LINII
// ============================================================================

/// Stan edytowanej linii (prosta implementacja readline-like).
pub struct LineBuffer {
    pub buf:    Vec<char>,
    pub cursor: usize,   // pozycja kursora (indeks w buf)
}

impl LineBuffer {
    pub fn new() -> Self {
        Self { buf: Vec::new(), cursor: 0 }
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.cursor = 0;
    }

    /// Wstaw znak w miejscu kursora.
    pub fn insert(&mut self, c: char) {
        self.buf.insert(self.cursor, c);
        self.cursor += 1;
    }

    /// Backspace — usuń znak przed kursorem.
    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.buf.remove(self.cursor);
        }
    }

    /// Delete — usuń znak pod kursorem.
    pub fn delete_at(&mut self) {
        if self.cursor < self.buf.len() {
            self.buf.remove(self.cursor);
        }
    }

    pub fn move_left(&mut self)  { if self.cursor > 0 { self.cursor -= 1; } }
    pub fn move_right(&mut self) { if self.cursor < self.buf.len() { self.cursor += 1; } }
    pub fn move_home(&mut self)  { self.cursor = 0; }
    pub fn move_end(&mut self)   { self.cursor = self.buf.len(); }

    /// Konwertuj bufor do String.
    pub fn to_string(&self) -> String {
        self.buf.iter().collect()
    }

    /// Zastąp całą zawartość.
    pub fn set(&mut self, s: &str) {
        self.buf.clear();
        for c in s.chars() { self.buf.push(c); }
        self.cursor = self.buf.len();
    }
}

// ============================================================================
// PARSER LINII POLECEŃ
// ============================================================================

/// Token wynikowy parsowania.
#[derive(Debug, Clone)]
pub enum Token {
    Word(String),         // zwykłe słowo lub flaga
    Quoted(String),       // string w cudzysłowach (zachowuje spacje)
}

impl Token {
    pub fn as_str(&self) -> &str {
        match self {
            Token::Word(s)   => s,
            Token::Quoted(s) => s,
        }
    }
}

/// Parsuje linię komend na tokeny.
/// Obsługuje:
///   - Spacje jako separatory
///   - "tekst z spacjami" (cudzysłowy)
///   - 'tekst z spacjami' (apostrofy)
///   - Komentarze po #
pub fn parse_cmdline(line: &str) -> Vec<Token> {
    let mut tokens  = Vec::new();
    let mut current = String::new();
    let mut chars   = line.chars().peekable();
    let mut in_dq   = false; // double quote
    let mut in_sq   = false; // single quote

    while let Some(c) = chars.next() {
        match c {
            '#' if !in_dq && !in_sq => break, // komentarz — reszta linii ignorowana
            '"' if !in_sq => {
                if in_dq {
                    // zamknij double quote
                    tokens.push(Token::Quoted(current.clone()));
                    current.clear();
                    in_dq = false;
                } else {
                    in_dq = true;
                }
            }
            '\'' if !in_dq => {
                if in_sq {
                    tokens.push(Token::Quoted(current.clone()));
                    current.clear();
                    in_sq = false;
                } else {
                    in_sq = true;
                }
            }
            ' ' | '\t' if !in_dq && !in_sq => {
                if !current.is_empty() {
                    tokens.push(Token::Word(current.clone()));
                    current.clear();
                }
            }
            _ => { current.push(c); }
        }
    }

    // Flush ostatniego tokenu
    if !current.is_empty() {
        if in_dq || in_sq {
            tokens.push(Token::Quoted(current));
        } else {
            tokens.push(Token::Word(current));
        }
    }

    tokens
}

// ============================================================================
// STAN TERMINALA
// ============================================================================

pub struct TerminalState {
    /// Bieżący katalog roboczy (pełna ścieżka z prefiksem dysku, np. "!d1;/home/ctrl")
    pub cwd:        String,
    /// Nazwa użytkownika (wyświetlana w prompt)
    pub username:   String,
    /// Nazwa hosta
    pub hostname:   String,
    /// Historia komend
    pub history:    History,
    /// Bufor edycji bieżącej linii
    pub line_buf:   LineBuffer,
    /// Czy terminal jest aktywny
    pub running:    bool,
    /// Liczba wykonanych komend
    pub cmd_count:  u64,
    /// Czy wyświetlać timestamp w prompt
    pub show_time:  bool,
}

impl TerminalState {
    pub fn new() -> Self {
        Self {
            cwd:       String::from("!d1;/home/ctrl"),
            username:  String::from("ctrl"),
            hostname:  String::from("cosinus"),
            history:   History::new(),
            line_buf:  LineBuffer::new(),
            running:   true,
            cmd_count: 0,
            show_time: false,
        }
    }
}

// ============================================================================
// WYPISYWANIE
// ============================================================================

/// Wypisz prompt: ctrl@cosinus:!d1;/home/ctrl$
fn print_prompt(state: &TerminalState) {
    print(BR_GREEN); print(&state.username);
    print(WHITE);    print("@");
    print(BR_CYAN);  print(&state.hostname);
    print(WHITE);    print(":");
    print(BR_YELLOW);print(&state.cwd);
    print(BR_WHITE); print("$ ");
    print(RESET);
}

/// Wypisz error.
fn print_err(msg: &str) {
    print(BR_RED);
    print("error: ");
    print(RESET);
    println(msg);
}

/// Wypisz ostrzeżenie.
fn print_warn(msg: &str) {
    print(YELLOW);
    print("warn:  ");
    print(RESET);
    println(msg);
}

/// Wypisz info.
fn print_info(msg: &str) {
    print(BR_CYAN);
    print("  --> ");
    print(RESET);
    println(msg);
}

/// Wypisz sukces.
fn print_ok(msg: &str) {
    print(BR_GREEN);
    print("  [+] ");
    print(RESET);
    println(msg);
}

/// Opis błędu FsError.
fn fs_err_msg(e: FsError) -> &'static str {
    match e {
        FsError::NotFound        => "nie znaleziono pliku lub katalogu",
        FsError::NotAFile        => "to nie jest plik",
        FsError::NotADir         => "to nie jest katalog",
        FsError::AlreadyExists   => "plik lub katalog już istnieje",
        FsError::PermissionDenied=> "brak uprawnień",
        FsError::DiskFull        => "brak miejsca na dysku",
        FsError::IoError         => "błąd I/O",
        FsError::NotMounted      => "partycja nie jest podmontowana",
        FsError::BadDiskId       => "nieprawidłowy identyfikator dysku (dozwolone: ! @ # $ % ^ & *)",
        FsError::BadPartId       => "nieprawidłowy numer partycji (dozwolone: d1–d9)",
        FsError::InvalidPath     => "nieprawidłowa ścieżka",
        FsError::EndOfFile       => "koniec pliku",
        FsError::NotSupported    => "operacja nieobsługiwana",
        FsError::InvalidOffset   => "nieprawidłowe przesunięcie",
    }
}

// ============================================================================
// NORMALIZACJA ŚCIEŻEK
// ============================================================================

/// Zamień ścieżkę względną na bezwzględną względem cwd.
/// Jeśli ścieżka zaczyna się od znaku dysku (np. "!d1;/...") → zwróć bez zmian.
/// Jeśli zaczyna się od "/" → dokej prefix dysku z cwd.
/// Inaczej → sklejaj z cwd.
fn resolve_path(cwd: &str, input: &str) -> String {
    let input = input.trim();

    // Absolutna ścieżka z prefiksem dysku — już gotowa
    if input.len() >= 4 && input.as_bytes()[1] == b'd' && input.as_bytes()[3] == b';' {
        return String::from(input);
    }

    // Wyodrębnij prefiks dysku z cwd ("!d1")
    let prefix = &cwd[..4]; // np. "!d1;"

    if input.starts_with('/') {
        // Absolutna ścieżka w obrębie bieżącego dysku
        return format!("{}{}", prefix, input);
    }

    // Względna ścieżka — rozwiąż komponenty ".."
    // cwd = "!d1;/home/ctrl", input = "../../etc"
    let cwd_path = &cwd[4..]; // sama ścieżka bez prefiksu

    let mut components: Vec<&str> = cwd_path.split('/').filter(|s| !s.is_empty()).collect();

    for part in input.split('/') {
        match part {
            "" | "." => {}
            ".." => { components.pop(); }
            name => { components.push(name); }
        }
    }

    let resolved_path = if components.is_empty() {
        String::from("/")
    } else {
        let mut p = String::from("/");
        for (i, c) in components.iter().enumerate() {
            if i > 0 { p.push('/'); }
            p.push_str(c);
        }
        p
    };

    format!("{}{}", prefix, resolved_path)
}

// ============================================================================
// IMPLEMENTACJA KOMEND
// ============================================================================

fn cmd_ls(state: &TerminalState, args: &[&str]) {
    let target = if args.is_empty() {
        state.cwd.clone()
    } else {
        resolve_path(&state.cwd, args[0])
    };

    match dir_list(&target) {
        Ok(entries) => {
            if entries.is_empty() {
                print_info("(pusty katalog)");
                return;
            }

            // Nagłówek
            print(DIM); print(BR_BLACK);
            println("  typ   rozmiar  nazwa");
            println("  ───   ───────  ─────");
            print(RESET);

            let mut dirs  = 0usize;
            let mut files = 0usize;

            // Najpierw katalogi, potem pliki
            let mut sorted = entries.clone();
            sorted.sort_by(|a, b| {
                match (a.is_dir, b.is_dir) {
                    (true, false) => core::cmp::Ordering::Less,
                    (false, true) => core::cmp::Ordering::Greater,
                    _ => a.name.as_str().cmp(b.name.as_str()),
                }
            });

            for entry in &sorted {
                if entry.is_dir {
                    print(BR_BLUE);
                    print("  dir   ");
                    print(DIM); print("       - ");
                    print(RESET); print(BR_BLUE);
                    print(&entry.name);
                    println("/");
                    print(RESET);
                    dirs += 1;
                } else {
                    print(BR_WHITE);
                    print("  file  ");
                    print(DIM);
                    // Rozmiar z prawym wyrównaniem do 8 znaków
                    let size_str = format_size(entry.size);
                    let pad = 8usize.saturating_sub(size_str.len());
                    for _ in 0..pad { print(" "); }
                    print(&size_str);
                    print(" ");
                    print(RESET); print(WHITE);
                    println(&entry.name);
                    print(RESET);
                    files += 1;
                }
            }

            print(DIM); print(BR_BLACK);
            print_fmt!("\n  {} katalog(ów), {} plik(ów)\n", dirs, files);
            print(RESET);
        }
        Err(e) => print_err(fs_err_msg(e)),
    }
}

fn cmd_cd(state: &mut TerminalState, args: &[&str]) {
    if args.is_empty() {
        // cd bez argumentu → katalog domowy
        state.cwd = format!("{}d1;/home/{}", '!', state.username);
        return;
    }

    let target = resolve_path(&state.cwd, args[0]);

    // Sprawdź czy katalog istnieje
    match file_stat(&target) {
        Ok(stat) if stat.is_dir => {
            state.cwd = target;
        }
        Ok(_) => print_err("to nie jest katalog"),
        Err(e) => print_err(fs_err_msg(e)),
    }
}

fn cmd_pwd(state: &TerminalState) {
    print(BR_YELLOW);
    println(&state.cwd);
    print(RESET);
}

fn cmd_cat(state: &TerminalState, args: &[&str]) {
    if args.is_empty() {
        print_err("użycie: cat <plik>");
        return;
    }

    let path = resolve_path(&state.cwd, args[0]);

    match file_read_all(&path) {
        Ok(data) => {
            // Wypisz linię po linii z numerami
            let text = core::str::from_utf8(&data).unwrap_or("(dane binarne)");
            print(DIM); print(BR_BLACK);
            print_fmt!("  ── {} ──\n", path_basename(args[0]));
            print(RESET);

            let mut line_num = 1usize;
            for line in text.split('\n') {
                print(DIM); print(BR_BLACK);
                print_fmt!("{:4} │ ", line_num);
                print(RESET); print(WHITE);
                println(line);
                print(RESET);
                line_num += 1;
            }
        }
        Err(FsError::NotAFile) => {
            print_err("to jest katalog — użyj 'ls' aby wylistować zawartość");
        }
        Err(e) => print_err(fs_err_msg(e)),
    }
}

fn cmd_touch(state: &TerminalState, args: &[&str]) {
    if args.is_empty() {
        print_err("użycie: touch <plik>");
        return;
    }

    let path = resolve_path(&state.cwd, args[0]);
    let flags = OpenFlags::CREATE | OpenFlags::WRITE;

    match VFS.lock().as_mut() {
        Some(vfs) => {
            match vfs.open(&path, flags) {
                Ok(handle) => {
                    let _ = vfs.close(handle);
                    print_ok(&format!("utworzono: {}", args[0]));
                }
                Err(FsError::AlreadyExists) => {
                    print_info("plik już istnieje (timestamp odświeżony)");
                }
                Err(e) => print_err(fs_err_msg(e)),
            }
        }
        None => print_err("VFS niezainicjowany"),
    }
}

fn cmd_mkdir(state: &TerminalState, args: &[&str]) {
    if args.is_empty() {
        print_err("użycie: mkdir <katalog>");
        return;
    }

    // Obsługa flagi -p (twórz brakujące katalogi nadrzędne)
    let (make_parents, name_arg) = if args[0] == "-p" {
        if args.len() < 2 { print_err("użycie: mkdir -p <katalog>"); return; }
        (true, args[1])
    } else {
        (false, args[0])
    };

    let path = resolve_path(&state.cwd, name_arg);

    if make_parents {
        // Twórz po kolei każdy komponent ścieżki
        let prefix = &path[..4];
        let fs_path = &path[4..];
        let mut built = String::from(prefix);
        built.push('/');

        for component in fs_path.split('/').filter(|s| !s.is_empty()) {
            built.push_str(component);
            match dir_create(&built) {
                Ok(_) | Err(FsError::AlreadyExists) => {}
                Err(e) => { print_err(fs_err_msg(e)); return; }
            }
            built.push('/');
        }
        print_ok(&format!("utworzono drzewo: {}", name_arg));
    } else {
        match dir_create(&path) {
            Ok(_)                      => print_ok(&format!("katalog utworzony: {}", name_arg)),
            Err(FsError::AlreadyExists) => print_warn("katalog już istnieje"),
            Err(e)                     => print_err(fs_err_msg(e)),
        }
    }
}

fn cmd_rm(state: &TerminalState, args: &[&str]) {
    if args.is_empty() {
        print_err("użycie: rm <plik>");
        return;
    }

    let path = resolve_path(&state.cwd, args[0]);

    match file_stat(&path) {
        Ok(stat) if stat.is_dir => {
            print_err("to jest katalog — użyj 'rmdir' lub 'rm -r'");
            return;
        }
        _ => {}
    }

    match VFS.lock().as_mut() {
        Some(vfs) => match vfs.rm(&path) {
            Ok(_)  => print_ok(&format!("usunięto: {}", args[0])),
            Err(e) => print_err(fs_err_msg(e)),
        },
        None => print_err("VFS niezainicjowany"),
    }
}

fn cmd_rmdir(state: &TerminalState, args: &[&str]) {
    if args.is_empty() {
        print_err("użycie: rmdir <katalog>");
        return;
    }

    let path = resolve_path(&state.cwd, args[0]);

    match VFS.lock().as_mut() {
        Some(vfs) => match vfs.rmdir(&path) {
            Ok(_)                       => print_ok(&format!("usunięto katalog: {}", args[0])),
            Err(FsError::NotSupported)  => print_err("katalog nie jest pusty"),
            Err(e)                      => print_err(fs_err_msg(e)),
        },
        None => print_err("VFS niezainicjowany"),
    }
}

fn cmd_echo(args: &[&str]) {
    // Obsługa flag:
    //   -n  → bez nowej linii
    //   -e  → interpretacja \n \t (uproszczona)
    let mut no_newline = false;
    let mut escape     = false;
    let mut start      = 0usize;

    for (i, &a) in args.iter().enumerate() {
        match a {
            "-n" => { no_newline = true; start = i + 1; }
            "-e" => { escape = true;     start = i + 1; }
            _    => { start = i; break; }
        }
    }

    let text = args[start..].join(" ");
    let output = if escape {
        text.replace("\\n", "\n").replace("\\t", "\t").replace("\\033", "\x1b")
    } else {
        text
    };

    print(WHITE);
    print(&output);
    if !no_newline { println(""); }
    print(RESET);
}

fn cmd_write(state: &TerminalState, args: &[&str]) {
    if args.len() < 2 {
        print_err("użycie: write <plik> <tekst...>");
        return;
    }

    let path = resolve_path(&state.cwd, args[0]);
    let text = args[1..].join(" ");

    match file_write_all(&path, text.as_bytes()) {
        Ok(_)  => print_ok(&format!("zapisano {} bajtów → {}", text.len(), args[0])),
        Err(e) => print_err(fs_err_msg(e)),
    }
}

fn cmd_append(state: &TerminalState, args: &[&str]) {
    if args.len() < 2 {
        print_err("użycie: append <plik> <tekst...>");
        return;
    }

    let path  = resolve_path(&state.cwd, args[0]);
    let text  = args[1..].join(" ") + "\n";
    let flags = OpenFlags::APPEND | OpenFlags::WRITE | OpenFlags::CREATE;

    match VFS.lock().as_mut() {
        Some(vfs) => {
            match vfs.open(&path, flags) {
                Ok(mut handle) => {
                    match vfs.write(&mut handle, text.as_bytes()) {
                        Ok(n)  => {
                            let _ = vfs.close(handle);
                            print_ok(&format!("dołączono {} bajtów → {}", n, args[0]));
                        }
                        Err(e) => { let _ = vfs.close(handle); print_err(fs_err_msg(e)); }
                    }
                }
                Err(e) => print_err(fs_err_msg(e)),
            }
        }
        None => print_err("VFS niezainicjowany"),
    }
}

fn cmd_stat(state: &TerminalState, args: &[&str]) {
    if args.is_empty() {
        print_err("użycie: stat <ścieżka>");
        return;
    }

    let path = resolve_path(&state.cwd, args[0]);

    match file_stat(&path) {
        Ok(stat) => {
            print(BR_CYAN);   print("  Nazwa:    "); print(RESET); println(&stat.name);
            print(BR_CYAN);   print("  Typ:      "); print(RESET);
            println(if stat.is_dir { "katalog" } else { "plik" });
            print(BR_CYAN);   print("  Rozmiar:  "); print(RESET);
            println(&format_size(stat.size));
            print(BR_CYAN);   print("  Ścieżka:  "); print(RESET); println(&path);
        }
        Err(e) => print_err(fs_err_msg(e)),
    }
}

/// Komenda msg — wyświetla wiadomość w ozdobnej ramce.
/// Użycie: msg <tekst...>
/// Przykład: msg Witaj w CosinusOS!
fn cmd_msg(args: &[&str]) {
    if args.is_empty() {
        print_err("użycie: msg <wiadomość...>");
        return;
    }

    let text = args.join(" ");
    let len  = text.len();

    // Minimalna szerokość ramki
    let width = len.max(40);

    // Linia pozioma (═══...═══)
    let mut hline = String::new();
    for _ in 0..(width + 2) { hline.push('═'); }

    print(BR_YELLOW); print(BOLD);
    print("  ╔"); print(&hline); println("╗");
    print("  ║ "); print(RESET); print(BR_WHITE); print(BOLD);

    // Wyśrodkuj tekst
    let padding = (width - len) / 2;
    let rpad    = width - len - padding;
    for _ in 0..padding { print(" "); }
    print(&text);
    for _ in 0..rpad    { print(" "); }

    print(RESET); print(BR_YELLOW); print(BOLD);
    println(" ║");
    print("  ╚"); print(&hline); println("╝");
    print(RESET);
}

fn cmd_clear() {
    // ANSI: wyczyść ekran i przesuń kursor na początek
    print("\x1b[2J\x1b[H");
}

fn cmd_history(state: &TerminalState) {
    if state.history.len() == 0 {
        print_info("historia jest pusta");
        return;
    }
    print(DIM); print(BR_BLACK);
    println("  Historia komend:");
    println("  ──────────────────");
    print(RESET);
    for i in 0..state.history.len() {
        if let Some(cmd) = state.history.get(i) {
            print(BR_BLACK); print(DIM);
            print_fmt!("  {:3}  ", i + 1);
            print(RESET); print(WHITE);
            println(cmd);
            print(RESET);
        }
    }
}

fn cmd_help() {
    print(BR_CYAN); print(BOLD);
    println("  ┌──────────────────────────────────────────────────┐");
    println("  │         CosinusOS Terminal — Pomoc               │");
    println("  ├──────────────────────────────────────────────────┤");
    print(RESET);

    let commands = [
        ("Nawigacja", vec![
            ("ls [ścieżka]",          "listuj katalog"),
            ("cd <ścieżka>",          "zmień katalog (obsługuje ..  /)"),
            ("pwd",                   "pokaż bieżący katalog"),
        ]),
        ("Pliki", vec![
            ("cat <plik>",            "wyświetl zawartość pliku"),
            ("touch <plik>",          "utwórz pusty plik"),
            ("write <plik> <tekst>",  "zapisz tekst do pliku"),
            ("append <plik> <tekst>", "dołącz tekst do pliku"),
            ("stat <ścieżka>",        "informacje o pliku/katalogu"),
            ("rm <plik>",             "usuń plik"),
        ]),
        ("Katalogi", vec![
            ("mkdir <katalog>",       "utwórz katalog"),
            ("mkdir -p <katalog>",    "utwórz razem z katalogami nadrzędnymi"),
            ("rmdir <katalog>",       "usuń pusty katalog"),
        ]),
        ("Wyjście", vec![
            ("echo <tekst>",          "wypisz tekst (-n bez newline, -e escapowanie)"),
            ("msg <tekst>",           "wyświetl wiadomość w ozdobnej ramce"),
        ]),
        ("System", vec![
            ("history",               "historia komend"),
            ("clear",                 "wyczyść ekran"),
            ("help",                  "ta pomoc"),
            ("exit / quit",           "zakończ terminal"),
        ]),
    ];

    for (section, cmds) in &commands {
        print(BR_YELLOW); print(BOLD);
        print_fmt!("  │  {:<46}  │\n", format!("── {} ──", section));
        print(RESET);
        for (name, desc) in cmds {
            print(BR_WHITE);
            print_fmt!("  │    {:<20}", name);
            print(DIM); print(WHITE);
            print_fmt!("  {:<24}  │\n", desc);
            print(RESET);
        }
    }

    print(BR_CYAN); print(BOLD);
    println("  │                                                    │");
    println("  │  Ścieżki:  !d1;/home   @d2;/data   #d3;/backup    │");
    println("  │  Dyski:  ! @ # $ % ^ & *   Partycje: d1–d9        │");
    println("  └──────────────────────────────────────────────────┘");
    print(RESET);
}

// ============================================================================
// FORMATOWANIE
// ============================================================================

fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{}B", bytes)
    } else if bytes < 1024 * 1024 {
        format!("{:.1}K", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}M", bytes as f64 / 1024.0 / 1024.0)
    }
}

// ============================================================================
// DISPATCHER KOMEND
// ============================================================================

/// Wykonaj jedną linię polecenia. Zwraca false jeśli terminal ma się zakończyć.
pub fn execute(state: &mut TerminalState, line: &str) -> bool {
    let line = line.trim();
    if line.is_empty() { return true; }

    // Dodaj do historii
    state.history.push(String::from(line));
    state.cmd_count += 1;

    let tokens = parse_cmdline(line);
    if tokens.is_empty() { return true; }

    let args_str: Vec<&str> = tokens.iter().map(|t| t.as_str()).collect();
    let cmd  = args_str[0];
    let args = &args_str[1..];

    match cmd {
        // Nawigacja
        "ls" | "dir"  => cmd_ls(state, args),
        "cd"          => cmd_cd(state, args),
        "pwd"         => cmd_pwd(state),

        // Pliki
        "cat" | "type" => cmd_cat(state, args),
        "touch"        => cmd_touch(state, args),
        "write"        => cmd_write(state, args),
        "append"       => cmd_append(state, args),
        "stat"         => cmd_stat(state, args),
        "rm" | "del"   => cmd_rm(state, args),

        // Katalogi
        "mkdir" | "md" => cmd_mkdir(state, args),
        "rmdir" | "rd" => cmd_rmdir(state, args),

        // Wyjście i wiadomości
        "echo" | "print" => cmd_echo(args),
        "msg"            => cmd_msg(args),

        // System
        "history"        => cmd_history(state),
        "clear" | "cls"  => cmd_clear(),
        "help" | "?"     => cmd_help(),
        "exit" | "quit"  => {
            print(BR_CYAN);
            println("\n  Do widzenia!\n");
            print(RESET);
            state.running = false;
            return false;
        }

        // Nieznana komenda
        unknown => {
            print(BR_RED);
            print("  nieznana komenda: ");
            print(RESET); print(WHITE);
            println(unknown);
            print(DIM);
            println("  wpisz 'help' aby zobaczyć listę komend");
            print(RESET);
        }
    }

    true
}

// ============================================================================
// GŁÓWNA PĘTLA TERMINALA
// ============================================================================

/// Uruchom terminal w trybie interaktywnym.
/// `input_fn` — funkcja dostarczająca kolejne linie (pozwala wstrzyknąć
///              skrypt lub symulowany input w testach).
pub fn run_terminal<F>(mut input_fn: F)
where
    F: FnMut() -> Option<String>,
{
    let mut state = TerminalState::new();

    // Banner
    print(BR_CYAN); print(BOLD);
    println("");
    println("  ╔══════════════════════════════════════╗");
    println("  ║     CosinusOS Terminal v1.0          ║");
    println("  ║     wpisz 'help' aby zobaczyć pomoc  ║");
    println("  ╚══════════════════════════════════════╝");
    print(RESET);
    println("");

    while state.running {
        // Wyświetl prompt
        print_prompt(&state);

        // Pobierz linię
        match input_fn() {
            Some(line) => {
                if !execute(&mut state, &line) { break; }
            }
            None => break, // EOF
        }
    }
}

// ============================================================================
// WERSJA DEMONSTRACYJNA — uruchamia gotowy skrypt komend
// ============================================================================

/// Uruchom terminal z predefiniowanym skryptem demo.
/// Używane przy braku prawdziwego wejścia z klawiatury (etap boot).
pub fn run_demo() {
    // Upewnij się że VFS jest zainicjowany
    crate::files::file_system();

    // Skrypt komend do wykonania
    let script: &[&str] = &[
        "msg Witaj w CosinusOS!",
        "pwd",
        "ls",
        "mkdir -p !d1;/home/ctrl/desktop",
        "mkdir !d1;/home/ctrl/dokumenty",
        "mkdir !d1;/home/ctrl/pobrane",
        "touch !d1;/home/ctrl/desktop/notatka.txt",
        "write !d1;/home/ctrl/desktop/notatka.txt Pierwsza notatka w CosinusOS",
        "cat !d1;/home/ctrl/desktop/notatka.txt",
        "append !d1;/home/ctrl/desktop/notatka.txt Druga linia notatki",
        "cat !d1;/home/ctrl/desktop/notatka.txt",
        "stat !d1;/home/ctrl/desktop/notatka.txt",
        "ls !d1;/home/ctrl",
        "cd !d1;/home/ctrl/desktop",
        "pwd",
        "ls",
        "write notatka.txt Nadpisano plik przez ścieżkę względną",
        "cat notatka.txt",
        "echo",
        "echo -e \\033[92mKolory ANSI działają!\\033[0m",
        "msg Wszystkie systemy sprawne!",
        "cd ..",
        "pwd",
        "history",
        "help",
    ];

    let mut idx = 0usize;
    run_terminal(|| {
        if idx < script.len() {
            let line = String::from(script[idx]);
            idx += 1;
            // Echo komendy (symulacja wpisywania)
            print(BR_GREEN); print("$ "); print(RESET);
            print(WHITE); println(&line); print(RESET);
            Some(line)
        } else {
            None
        }
    });
}

// ============================================================================
// PUBLICZNY ENTRY POINT (zgodny z sygnaturą w main.rs)
// ============================================================================

/// Zainicjuj i uruchom terminal CosinusOS.
/// Wywołaj z main() — blokuje aż do wpisania 'exit'.
pub fn terminal_main() {
    crate::files::file_system();
    run_demo(); // zamień na run_terminal(keyboard_input) gdy będzie sterownik klawiatury
}