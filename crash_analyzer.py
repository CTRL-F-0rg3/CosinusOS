#!/usr/bin/env python3
"""
CosinusOS Auto Crash Analyzer
Samo buduje, odpala QEMU, zbiera logi i analizuje crash.

Użycie (z katalogu projektu):
  python3 crash_analyzer.py
  python3 crash_analyzer.py --no-build          # pomiń zig build
  python3 crash_analyzer.py --iso path/to/cosinus.iso
  python3 crash_analyzer.py --timeout 15        # ile sekund czekać na boot
"""

import re, sys, os, argparse, subprocess, threading, time, signal, shutil
from pathlib import Path
from dataclasses import dataclass, field
from typing import Optional
from enum import Enum, auto

# ──────────────────────────────────────────────────────────────────────────────
# KONFIGURACJA — dostosuj do swojego projektu
# ──────────────────────────────────────────────────────────────────────────────

# Skrypt próbuje auto-wykryć te ścieżki, możesz nadpisać
DEFAULT_CONFIG = {
    "project_root":  None,   # auto-detect: szuka build.zig w górę od CWD
    "iso_path":      None,   # auto-detect: szuka *.iso w projekcie
    "elf_path":      None,   # auto-detect: szuka kernel.elf w build/
    "build_cmd":     None,   # auto-detect: zig build lub make
    "qemu_bin":      "qemu-system-x86_64",
    "boot_timeout":  12,     # sekund zanim uznamy że się zawiesił
    "qemu_ram":      "256M",
}

QEMU_FLAGS = [
    "-m",            "{ram}",
    "-cdrom",        "{iso}",
    "-serial",       "stdio",          # serial → nasz stdout/parse
    "-debugcon",     "file:/dev/stderr", # port 0xE9 → stderr
    "-no-reboot",                      # nie restartuj po crash
    "-d",            "int,cpu_reset",  # logi CPU do stderr
    "-D",            "/tmp/cosinus_qemu_int.log",
    "-display",      "none",           # headless
]

# ──────────────────────────────────────────────────────────────────────────────
# ANSI
# ──────────────────────────────────────────────────────────────────────────────
R   = "\033[1;31m";  Y  = "\033[1;33m"; G  = "\033[1;32m"
C   = "\033[1;36m";  W  = "\033[1;37m"; DG = "\033[2;37m"
BRD = "\033[41;1;37m"; RS = "\033[0m"

def clr(col, s): return f"{col}{s}{RS}"
def ok(s):    return clr(G,  f"[ OK ] {s}")
def err(s):   return clr(R,  f"[ERR ] {s}")
def warn(s):  return clr(Y,  f"[WARN] {s}")
def info(s):  return clr(C,  f"[INFO] {s}")
def fatal(s): return clr(BRD,f"[FATAL] {s}")
def hdr(s):
    w = 72
    print(f"\n{W}{'━'*w}{RS}")
    print(f"{W}  {s}{RS}")
    print(f"{W}{'━'*w}{RS}")

# ──────────────────────────────────────────────────────────────────────────────
# AUTO-DETECT PROJECT
# ──────────────────────────────────────────────────────────────────────────────

def find_project_root() -> Optional[Path]:
    """Szuka katalogu z build.zig lub Makefile idąc w górę od CWD."""
    p = Path.cwd()
    for _ in range(8):
        if (p / "build.zig").exists() or (p / "Makefile").exists():
            return p
        # Szukaj też w podkatalogach (src/kernel/build.zig)
        for sub in p.rglob("build.zig"):
            return sub.parent
        p = p.parent
    return None

def find_iso(root: Path) -> Optional[Path]:
    for pat in ["**/*.iso", "**/*.img"]:
        hits = list(root.glob(pat))
        if hits:
            return hits[0]
    return None

def find_elf(root: Path) -> Optional[Path]:
    for pat in ["**/kernel.elf", "**/build/kernel.elf"]:
        hits = list(root.glob(pat))
        if hits:
            return hits[0]
    return None

def find_build_cmd(root: Path) -> Optional[list]:
    # Szukaj build.zig w src/kernel
    kg = root / "src" / "kernel" / "build.zig"
    if kg.exists():
        return ["sh", "-c", f"cd {kg.parent} && zig build"]
    if (root / "build.zig").exists():
        return ["zig", "build"]
    if (root / "Makefile").exists():
        return ["make"]
    return None

# ──────────────────────────────────────────────────────────────────────────────
# PATTERNS
# ──────────────────────────────────────────────────────────────────────────────

PAT = {
    "boot_start":   re.compile(r"=== CosinusOS v[\d.]+ boot ==="),
    "boot_done":    re.compile(r"\[OK\] boot complete"),
    "system_ready": re.compile(r"SYSTEM GOTOWY"),
    "ok_step":      re.compile(r"\[ OK \]"),
    "err_step":     re.compile(r"\[ERR!\]"),
    "krsp":         re.compile(r"\[DBG\] krsp=(0x[0-9A-Fa-f]+)\s+kt=(0x[0-9A-Fa-f]+)"),
    "entry":        re.compile(r"\[DBG\] r14\(entry\)=(0x[0-9A-Fa-f]+)\s+r13\(ut\)=(0x[0-9A-Fa-f]+)"),
    "tramp":        re.compile(r"\[DBG\] tramp=(0x[0-9A-Fa-f]+)\s+expected[^=]*=(0x[0-9A-Fa-f]+)"),
    "tramp_ok":     re.compile(r"\[DBG\] tramp=.+\bOK\b"),
    "tramp_bad":    re.compile(r"MISMATCH|stos zepsuly"),
    "fatal":        re.compile(r"\[FATAL\]"),
    "pf":           re.compile(r"\[#PF\] PAGE FAULT"),
    "pf_detail":    re.compile(r"addr=(0x[0-9A-Fa-f]+)\s+err=(0x[0-9A-Fa-f]+)\s+rip=(0x[0-9A-Fa-f]+)"),
    "pf_kind":      re.compile(r"(USR|KRN)\s+(W|R)"),
    "df":           re.compile(r"\[#DF\].*RIP=(0x[0-9A-Fa-f]+)"),
    "panic":        re.compile(r"KERNEL PANIC"),
    "panic_msg":    re.compile(r"KERNEL PANIC \*{3}\s*\n?\s*(.+?)(?:\s*@|\s*\n|$)"),
    "elf":          re.compile(r"\[US\] ELF64\s+(ET_\w+)\s+entry=(0x[0-9A-Fa-f]+)"),
    "segment":      re.compile(r"\[SEG\] vaddr=(0x[0-9A-Fa-f]+)\s+filesz=(\d+)\s+memsz=(\d+)"),
    "us_ok":        re.compile(r"\[US\] Watek #(\d+) OK"),
    "us_fail":      re.compile(r"\[US\] Brak slotow"),
    "thread":       re.compile(r"\[T#(\d+)\]\s+(\S+)"),
    "triple_fault": re.compile(r"Triple fault|triple fault|CPU Reset|cpu reset", re.I),
    "qemu_gp":      re.compile(r"#GP|General Protection", re.I),
    "tramp_port":   re.compile(r"TRAMP_U"),
    "oom":          re.compile(r"OOM|Out of Memory", re.I),
}

# ──────────────────────────────────────────────────────────────────────────────
# STATE
# ──────────────────────────────────────────────────────────────────────────────

@dataclass
class State:
    lines:         list[str]        = field(default_factory=list)
    boot_started:  bool             = False
    boot_done:     bool             = False
    system_ready:  bool             = False
    scheduler_ok:  bool             = False
    userspace_ok:  bool             = False
    tramp_addr:    Optional[int]    = None
    tramp_expect:  Optional[int]    = None
    tramp_match:   Optional[bool]   = None
    entry:         Optional[int]    = None
    user_rsp:      Optional[int]    = None
    krsp:          Optional[int]    = None
    kt:            Optional[int]    = None
    pf:            Optional[dict]   = None
    df_rip:        Optional[int]    = None
    panic_msg:     Optional[str]    = None
    triple_fault:  bool             = False
    tramp_port_ok: bool             = False
    findings:      list[tuple]      = field(default_factory=list)  # (level, cat, msg, hint)

    def add(self, level, cat, msg, hint=None):
        self.findings.append((level, cat, msg, hint))

def h(s): return int(s, 16)

def parse_line(line: str, st: State):
    ln = line.strip()
    st.lines.append(ln)

    if PAT["boot_start"].search(ln):
        st.boot_started = True
        st.add("ok", "BOOT", "Boot sekwencja startuje")

    if PAT["boot_done"].search(ln):
        st.boot_done = True
        st.add("ok", "BOOT", "Boot zakończony — [OK] boot complete")

    if PAT["system_ready"].search(ln):
        st.system_ready = True
        st.add("ok", "BOOT", "System gotowy")

    if PAT["err_step"].search(ln):
        st.add("error", "BOOT", f"Krok boot zakończony ERR: {ln}",
               "Sprawdź poprzednie linie — co nie zainicjalizowało się")

    m = PAT["thread"].search(ln)
    if m:
        st.add("info", "THREAD", f"Wątek #{m.group(1)} '{m.group(2)}' utworzony")
        if "kterminal" in m.group(2) or "idle" in m.group(2):
            st.scheduler_ok = True

    m = PAT["krsp"].search(ln)
    if m:
        st.krsp, st.kt = h(m.group(1)), h(m.group(2))
        frame = st.kt - st.krsp
        if frame == 56:
            st.add("ok",   "TRAMP", f"krsp={m.group(1)} kt={m.group(2)} frame=56B ✓")
        elif st.krsp >= st.kt:
            st.add("fatal","TRAMP", f"krsp >= kt — stos odwrócony lub zerowy!",
                   "init_thread_stack nie dekrementuje ksp lub kt=0")
        else:
            st.add("warn", "TRAMP", f"krsp={m.group(1)} kt={m.group(2)} frame={frame}B (oczekiwano 56)")

    m = PAT["entry"].search(ln)
    if m:
        st.entry, st.user_rsp = h(m.group(1)), h(m.group(2))
        if st.entry == 0:
            st.add("fatal","TRAMP","entry=0x0 — userspace nie załadowany lub ELF nie sparsowany",
                   "Sprawdź load_userspace() i moduł MB2 w grub.cfg")
        else:
            st.add("ok","TRAMP", f"entry={m.group(1)}  user_rsp={m.group(2)}")
        if st.user_rsp == 0:
            st.add("fatal","TRAMP","user_rsp=0x0 — stos userspace nie zmapowany",
                   "spawn_user_on_cr3() nie zmapowało stosu lub utop=0")

    m = PAT["tramp"].search(ln)
    if m:
        st.tramp_addr, st.tramp_expect = h(m.group(1)), h(m.group(2))
        st.tramp_match = (st.tramp_addr == st.tramp_expect)
        if st.tramp_match:
            st.add("ok","TRAMP", f"Adres trampoliny MATCH: {m.group(1)} ✓")
        else:
            st.add("fatal","TRAMP",
                   f"MISMATCH trampoliny! stos={m.group(1)} symbol={m.group(2)}",
                   "tramp.o nie zlinkowany lub init_thread_stack używa złej metody pobierania adresu. "
                   "Sprawdź: nm ../../build/kernel.elf | grep tramp_u")
        if st.tramp_addr == 0:
            st.add("fatal","TRAMP","tramp_addr=0x0 — trampolina nie zlinkowana!",
                   "1) nm ../../build/tramp.o | grep tramp_u  "
                   "2) Sprawdź build.zig — tramp.o musi być w komendzie ld")

    if PAT["tramp_bad"].search(ln):
        st.add("fatal","TRAMP", f"Stos wątku zepsuty: {ln}",
               "init_thread_stack — ksp nie zmniejsza się poprawnie")

    if PAT["tramp_port"].search(ln):
        st.tramp_port_ok = True
        st.add("ok","TRAMP","tramp_u wywołana (port 0xE9 output) — trampolina odpala się",
               "Problem musi być po wejściu do trampoliny: iretq frame lub mapowanie")

    m = PAT["elf"].search(ln)
    if m:
        st.add("info","USERSPACE", f"ELF64 {m.group(1)} entry={m.group(2)}")

    m = PAT["segment"].search(ln)
    if m:
        vaddr, filesz, memsz = h(m.group(1)), int(m.group(2)), int(m.group(3))
        if vaddr == 0:
            st.add("error","SEGMENT","Segment pod vaddr=0x0 — NULL pointer segment!",
                   "ET_EXEC z absolutnymi adresami? Sprawdź load_base w load_userspace()")
        elif memsz == 2*1024*1024:
            st.add("warn","SEGMENT", f"Segment vaddr={m.group(1)} memsz obcięty do 2MB",
                   "BSS może być za duży — limit w load_userspace()")

    m = PAT["us_ok"].search(ln)
    if m:
        st.userspace_ok = True
        st.add("ok","USERSPACE", f"Wątek userspace #{m.group(1)} uruchomiony")

    if PAT["us_fail"].search(ln):
        st.add("error","USERSPACE","Brak wolnych slotów wątków",
               "MAX_THREADS wyczerpany — sprawdź czy wątki kończą się (state=Dead)")

    if PAT["pf"].search(ln):
        st.add("error","FAULT","#PF PAGE FAULT wykryty")

    m = PAT["pf_detail"].search(ln)
    if m:
        addr, err_code, rip = h(m.group(1)), h(m.group(2)), h(m.group(3))
        st.pf = {"addr": addr, "err": err_code, "rip": rip}
        present = bool(err_code & 1)
        write   = bool(err_code & 2)
        user    = bool(err_code & 4)
        instr   = bool(err_code & 16)
        desc = ("niezmapowana" if not present else "protection") + \
               (" W" if write else " R") + \
               (" ring3" if user else " ring0") + \
               (" INSTR_FETCH" if instr else "")
        hint = _pf_hint(addr, err_code, rip, st)
        st.add("fatal","FAULT",f"#PF addr={m.group(1)} rip={m.group(3)} [{desc}]", hint)

    m = PAT["df"].search(ln)
    if m:
        st.df_rip = h(m.group(1))
        hint = "#DF = poprzedni wyjątek nie mógł być obsłużony. "
        if st.pf:
            hint += f"Poprzedni #PF @ {hex(st.pf['addr'])} spowodował #DF. "
        if st.df_rip == 0:
            hint += "RIP=0 → ret z thread_switch skoczył pod 0x0 — tramp_addr na stosie był 0!"
        else:
            hint += "Sprawdź TSS.rsp0 i czy kernel stack jest zmapowany."
        st.add("fatal","FAULT",f"#DF DOUBLE FAULT @ RIP={m.group(1)}", hint)

    if PAT["panic"].search(ln):
        m2 = PAT["panic_msg"].search(ln)
        msg = m2.group(1).strip() if m2 else ln
        st.panic_msg = msg
        st.add("fatal","PANIC", f"KERNEL PANIC: {msg}", _panic_hint(msg))

    if PAT["triple_fault"].search(ln):
        st.triple_fault = True
        st.add("fatal","QEMU","Triple fault / CPU reset",
               "Sprawdź IST1 stack, TSS.ist1 != 0. "
               "Lub: -d int,cpu_reset 2>qemu_int.log dla więcej info")

    if PAT["qemu_gp"].search(ln):
        st.add("fatal","QEMU","#GP General Protection Fault",
               "Zły CS/SS selector w iretq? CS=0x1B SS=0x23. "
               "Lub null pointer / misaligned access w ring0")

    if PAT["oom"].search(ln):
        st.add("fatal","PMM","Out of Memory — PMM wyczerpał ramki",
               "Zwiększ MEM_SIZE w mm_init() lub zmniejsz KERNEL_STACK_SIZE/USER_STACK_SIZE")

def _pf_hint(addr, err, rip, st: State) -> str:
    p = []
    if addr < 0x1000:
        p.append("NULL ptr dereference")
    if st.entry and addr == st.entry and not (err & 1):
        p.append(f"Entry point {hex(addr)} nie zmapowany — brak PTE_U lub vmap() nie wywołane dla kodu")
    if st.tramp_addr and addr == st.tramp_addr:
        p.append("PF pod adresem trampoliny — trampolina nie zmapowana w cr3 wątku")
    if rip == 0:
        p.append("RIP=0 — skok pod NULL, tramp_addr na stosie był 0x0")
    if rip and rip < 0x101000:
        p.append(f"RIP={hex(rip)} < 0x101000 (baza kernela) — skok do złego adresu")
    if not (err & 1) and (err & 4):
        p.append("Strona userspace nie zmapowana z PTE_U — sprawdź vmap() w load_userspace()")
    if not p:
        p.append("Sprawdź mapowanie stron — czy vmap() używa właściwego cr3?")
    return " | ".join(p)

def _panic_hint(msg: str) -> str:
    m = msg.lower()
    if "oom" in m:             return "Więcej RAM w mm_init() lub mniejsze stosy"
    if "page fault" in m:      return "Sprawdź mapowanie stron przed crashem"
    if "tramp" in m or "stos" in m: return "Zły adres trampoliny lub uszkodzony stos wątku"
    return "Sprawdź logi powyżej panic"

# ──────────────────────────────────────────────────────────────────────────────
# NM SYMBOL LOOKUP
# ──────────────────────────────────────────────────────────────────────────────

def nm_lookup(elf: Path, addr: int) -> Optional[str]:
    try:
        out = subprocess.check_output(["nm", "-n", str(elf)],
                                      stderr=subprocess.DEVNULL, text=True)
        best, best_a = None, 0
        for line in out.splitlines():
            p = line.split()
            if len(p) < 3: continue
            try: a = int(p[0], 16)
            except: continue
            if a <= addr and a > best_a:
                best_a, best = a, p[-1]
        if best:
            off = addr - best_a
            return f"{best}+0x{off:x}" if off else best
    except: pass
    return None

def nm_check_tramps(elf: Path) -> dict:
    found = {}
    try:
        out = subprocess.check_output(["nm", str(elf)],
                                      stderr=subprocess.DEVNULL, text=True)
        for line in out.splitlines():
            p = line.split()
            if len(p) < 3: continue
            name = p[-1]
            if name in ("tramp_u", "tramp_k"):
                try: found[name] = int(p[0], 16)
                except: pass
    except: pass
    return found

# ──────────────────────────────────────────────────────────────────────────────
# BUILD
# ──────────────────────────────────────────────────────────────────────────────

def run_build(cmd: list, cwd: Path) -> bool:
    print(f"\n{C}[BUILD]{RS} {' '.join(cmd)}")
    try:
        proc = subprocess.run(cmd, cwd=str(cwd), timeout=120,
                              capture_output=False)
        if proc.returncode != 0:
            print(err(f"Build zakończony kodem {proc.returncode}"))
            return False
        print(ok("Build OK"))
        return True
    except subprocess.TimeoutExpired:
        print(err("Build timeout (120s)"))
        return False
    except FileNotFoundError as e:
        print(err(f"Komenda nie znaleziona: {e}"))
        return False

# ──────────────────────────────────────────────────────────────────────────────
# QEMU RUNNER
# ──────────────────────────────────────────────────────────────────────────────

def run_qemu(qemu_bin: str, iso: Path, ram: str, timeout: int) -> tuple[list[str], int]:
    """Uruchamia QEMU, zbiera logi z serial stdio, zwraca (lines, returncode)."""
    flags = [f.format(ram=ram, iso=str(iso)) for f in QEMU_FLAGS]
    cmd   = [qemu_bin] + flags
    print(f"\n{C}[QEMU]{RS} {' '.join(cmd[:6])} ...")

    lines   = []
    lock    = threading.Lock()
    done    = threading.Event()
    proc    = None
    rc      = [-1]

    def reader(stream, label):
        for raw in stream:
            line = raw.rstrip("\n").rstrip("\r")
            if not line: continue
            with lock:
                lines.append(line)
            # Koloruj live output
            col = DG
            if any(p.search(line) for p in [PAT["pf"], PAT["df"], PAT["panic"]]):
                col = R
            elif PAT["ok_step"].search(line):
                col = G
            elif any(p.search(line) for p in [PAT["tramp"], PAT["entry"], PAT["krsp"]]):
                col = C
            print(f"  {col}{label}{RS} {line}")

    try:
        proc = subprocess.Popen(
            cmd,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True, bufsize=1,
        )

        t_out = threading.Thread(target=reader, args=(proc.stdout, "SER"), daemon=True)
        t_err = threading.Thread(target=reader, args=(proc.stderr, "DBG"), daemon=True)
        t_out.start(); t_err.start()

        deadline = time.time() + timeout
        while time.time() < deadline:
            if proc.poll() is not None:
                break
            with lock:
                snap = "\n".join(lines)
            # Jeśli system gotowy — daj jeszcze 2s i zabij
            if PAT["system_ready"].search(snap) or PAT["boot_done"].search(snap):
                time.sleep(2)
                break
            # Jeśli crash — stop od razu
            if any(PAT[k].search(snap) for k in ["df","panic","triple_fault"]):
                time.sleep(0.5)
                break
            time.sleep(0.2)

        if proc.poll() is None:
            proc.terminate()
            try: proc.wait(timeout=3)
            except: proc.kill()

        t_out.join(timeout=2); t_err.join(timeout=2)
        rc[0] = proc.returncode if proc.returncode is not None else -1

    except FileNotFoundError:
        print(err(f"QEMU nie znaleziony: {qemu_bin}"))
        print(warn(f"Zainstaluj: sudo pacman -S qemu-system-x86  (Arch)"))
        print(warn(f"            sudo apt install qemu-system-x86 (Ubuntu)"))
        sys.exit(1)
    except Exception as e:
        print(err(f"QEMU błąd: {e}"))

    # Doczytaj logi qemu_int.log
    try:
        with open("/tmp/cosinus_qemu_int.log") as f:
            for line in f:
                line = line.strip()
                if line:
                    lines.append(f"[QEMU_INT] {line}")
    except: pass

    return lines, rc[0]

# ──────────────────────────────────────────────────────────────────────────────
# REPORT
# ──────────────────────────────────────────────────────────────────────────────

LEVEL_COL = {"ok": G, "info": C, "warn": Y, "error": R, "fatal": BRD}
LEVEL_LBL = {"ok": " OK  ", "info": "INFO ", "warn": "WARN ", "error": "ERR  ", "fatal": "FATAL"}

def print_report(st: State, elf: Optional[Path]):
    hdr("ANALIZA CRASHU — CosinusOS")

    # ── Boot summary ──────────────────────────────────────────────
    print(f"\n{W}═ BOOT STATUS{RS}")
    def s(b): return clr(G,"✓ OK") if b else clr(R,"✗ FAIL")
    print(f"  Boot start:       {s(st.boot_started)}")
    print(f"  Boot complete:    {s(st.boot_done)}")
    print(f"  System gotowy:    {s(st.system_ready)}")
    print(f"  Scheduler:        {s(st.scheduler_ok)}")
    print(f"  Userspace wątek:  {s(st.userspace_ok)}")

    # ── Trampoline ────────────────────────────────────────────────
    print(f"\n{W}═ TRAMPOLINA{RS}")
    if st.tramp_addr is not None:
        mc = G if st.tramp_match else R
        ms = "MATCH ✓" if st.tramp_match else "MISMATCH ← GŁÓWNA PRZYCZYNA"
        print(f"  Na stosie:   {clr(mc, hex(st.tramp_addr))}")
        print(f"  Oczekiwany:  {hex(st.tramp_expect)}")
        print(f"  Status:      {clr(mc, ms)}")
    else:
        print(f"  {Y}Brak danych [DBG] tramp= w logu{RS}")

    if st.krsp:
        fr = st.kt - st.krsp
        fc = G if fr == 56 else R
        print(f"  Stos frame:  krsp={hex(st.krsp)} kt={hex(st.kt)} {clr(fc, str(fr)+'B')} (oczekiwano 56B)")

    if st.entry:
        ec = G if st.entry else R
        rc = G if st.user_rsp else R
        print(f"  entry:       {clr(ec, hex(st.entry))}")
        print(f"  user_rsp:    {clr(rc, hex(st.user_rsp))}")

    if st.tramp_port_ok:
        print(f"  Port 0xE9:   {clr(G,'tramp_u wywołana ✓')} — problem po trampolinie (iretq/mapowanie)")

    # ── ELF symbols ──────────────────────────────────────────────
    if elf and elf.exists():
        print(f"\n{W}═ ELF SYMBOLS{RS}")
        syms = nm_check_tramps(elf)
        if "tramp_u" in syms:
            match_elf = (st.tramp_expect == syms["tramp_u"]) if st.tramp_expect else None
            mc = G if match_elf else (Y if match_elf is None else R)
            print(f"  tramp_u = {clr(mc, hex(syms['tramp_u']))}")
        else:
            print(f"  {clr(BRD,'tramp_u NIE MA W ELF!')}  → tramp.o nie zlinkowany")
            print(f"  {DG}Sprawdź: nm {elf} | grep tramp{RS}")
        if "tramp_k" in syms:
            print(f"  tramp_k = {clr(G, hex(syms['tramp_k']))}")

        # Resolve crash adresy
        for label, addr in [
            ("PF addr", st.pf["addr"] if st.pf else None),
            ("PF rip",  st.pf["rip"]  if st.pf else None),
            ("DF rip",  st.df_rip),
        ]:
            if addr and addr > 0x1000:
                sym = nm_lookup(elf, addr)
                if sym:
                    print(f"  {label} {hex(addr)} → {clr(Y, sym)}")

    # ── Findings ──────────────────────────────────────────────────
    fatals = [(l,c,m,h) for l,c,m,h in st.findings if l == "fatal"]
    errors = [(l,c,m,h) for l,c,m,h in st.findings if l == "error"]
    warns  = [(l,c,m,h) for l,c,m,h in st.findings if l == "warn"]
    others = [(l,c,m,h) for l,c,m,h in st.findings if l in ("ok","info")]

    if fatals or errors:
        print(f"\n{W}═ BŁĘDY KRYTYCZNE{RS}")
        for l,c,m,h in fatals + errors:
            col = LEVEL_COL[l]
            print(f"  {col}[{LEVEL_LBL[l]}]{RS} {W}{c}{RS}  {m}")
            if h: print(f"  {DG}         ↳ {h}{RS}")

    if warns:
        print(f"\n{W}═ OSTRZEŻENIA{RS}")
        for l,c,m,h in warns:
            print(f"  {Y}[{LEVEL_LBL[l]}]{RS} {c}  {m}")
            if h: print(f"  {DG}         ↳ {h}{RS}")

    if others:
        print(f"\n{W}═ PRZEBIEG BOOTU{RS}")
        for l,c,m,h in others:
            col = LEVEL_COL[l]
            print(f"  {col}[{LEVEL_LBL[l]}]{RS} {c}  {m}")

    # ── Diagnoza ──────────────────────────────────────────────────
    print(f"\n{W}═ DIAGNOZA I NASTĘPNY KROK{RS}")
    _diagnose(st, elf)

    print(f"\n{W}{'━'*72}{RS}\n")

def _diagnose(st: State, elf: Optional[Path]):
    if not st.boot_started:
        print(f"  {R}Kernel w ogóle nie startuje.{RS}")
        print(f"  {DG}Sprawdź boot.asm, linker.ld i czy GRUB widzi moduł{RS}")
        return

    if st.tramp_match is False:
        tramp_zero = st.tramp_addr == 0
        print(f"  {clr(BRD,'GŁÓWNA PRZYCZYNA: Zły adres trampoliny na stosie wątku')}")
        if tramp_zero:
            print(f"\n  {W}tramp_addr = 0x0 — trampolina nie zlinkowana.{RS}")
            print(f"  {Y}Co sprawdzić:{RS}")
            print(f"  {DG}  1. nm ../../build/tramp.o | grep tramp_u{RS}")
            print(f"  {DG}     → jeśli brak: NASM nie skompilował tramp.asm poprawnie{RS}")
            print(f"  {DG}  2. nm ../../build/kernel.elf | grep tramp_u{RS}")
            print(f"  {DG}     → jeśli brak: tramp.o nie ma w komendzie ld w build.zig{RS}")
            print(f"  {DG}  3. grep 'tramp.o' src/kernel/build.zig{RS}")
        else:
            print(f"\n  {W}Adresy się nie zgadzają — stary ELF lub problem z linkerem.{RS}")
            print(f"  {DG}  1. zig build clean && zig build{RS}")
            print(f"  {DG}  2. nm ../../build/kernel.elf | grep tramp_u{RS}")
            print(f"  {DG}     i porównaj z 'expected' z logu{RS}")
        return

    if st.df_rip is not None:
        print(f"  {clr(BRD,'GŁÓWNA PRZYCZYNA: #DF Double Fault')}")
        if st.df_rip == 0:
            print(f"\n  {W}RIP=0 → thread_switch ret skoczył pod NULL{RS}")
            print(f"  {Y}Oznacza że tramp_addr na stosie był 0x0.{RS}")
            print(f"  {DG}  → sprawdź nm kernel.elf | grep tramp_u{RS}")
        elif st.pf:
            print(f"\n  {W}Poprzedni #PF @ {hex(st.pf['addr'])} wywołał #DF{RS}")
            print(f"  {DG}  Stos kernelowy przepełniony podczas obsługi #PF?{RS}")
            print(f"  {DG}  Sprawdź TSS.rsp0 i mapowanie kernel stacka{RS}")
        return

    if st.pf:
        addr, err_code, rip = st.pf["addr"], st.pf["err"], st.pf["rip"]
        print(f"  {clr(BRD,'GŁÓWNA PRZYCZYNA: #PF Page Fault')}")
        if addr == 0:
            print(f"\n  {W}NULL pointer dereference{RS}")
        elif not (err_code & 1) and (err_code & 4):
            print(f"\n  {W}Strona userspace nie zmapowana lub brak PTE_U{RS}")
            if st.entry and addr == st.entry:
                print(f"  {Y}Fault pod adresem entry={hex(st.entry)} — segment kodu nie załadowany?{RS}")
            print(f"  {DG}  Sprawdź load_userspace() — czy vmap() ma PTE_W | PTE_U{RS}")
            print(f"  {DG}  i czy używa cr3 wątku a nie K_P4{RS}")
        return

    if st.panic_msg:
        print(f"  {clr(BRD,f'KERNEL PANIC: {st.panic_msg}')}")
        return

    if st.triple_fault:
        print(f"  {clr(BRD,'Triple fault — CPU zresetował się')}")
        print(f"  {DG}  Uruchom QEMU z: -d int,cpu_reset 2>qemu_int.log{RS}")
        return

    if st.system_ready and st.userspace_ok:
        print(f"  {G}Brak crashy wykrytych. System wystartował pomyślnie.{RS}")
        return

    if st.boot_done and not st.userspace_ok:
        print(f"  {Y}Boot OK ale userspace nie uruchomiony.{RS}")
        print(f"  {DG}  Brak modułu MB2? Sprawdź grub.cfg: module2 /boot/userspace.bin{RS}")
        return

    print(f"  {Y}Brak jednoznacznej przyczyny w zebranych logach.{RS}")
    print(f"  {DG}  Dodaj do QEMU: -debugcon stdio  i uruchom tramp_debug.asm{RS}")

# ──────────────────────────────────────────────────────────────────────────────
# MAIN
# ──────────────────────────────────────────────────────────────────────────────

def main():
    ap = argparse.ArgumentParser(description="CosinusOS Auto Crash Analyzer")
    ap.add_argument("--no-build",  action="store_true",  help="Pomiń zig build")
    ap.add_argument("--iso",       help="Ścieżka do ISO (auto-detect jeśli pominięte)")
    ap.add_argument("--elf",       help="Ścieżka do kernel.elf (auto-detect)")
    ap.add_argument("--timeout",   type=int, default=DEFAULT_CONFIG["boot_timeout"],
                    help=f"Timeout QEMU w sekundach (domyślnie {DEFAULT_CONFIG['boot_timeout']})")
    ap.add_argument("--ram",       default=DEFAULT_CONFIG["qemu_ram"],
                    help="RAM dla QEMU (domyślnie 256M)")
    ap.add_argument("--qemu",      default=DEFAULT_CONFIG["qemu_bin"],
                    help="Ścieżka do qemu-system-x86_64")
    args = ap.parse_args()

    hdr("CosinusOS Auto Crash Analyzer")

    # ── Znajdź projekt ────────────────────────────────────────────
    root = find_project_root()
    if root:
        print(ok(f"Projekt: {root}"))
    else:
        print(warn("Nie znaleziono katalogu projektu (build.zig) — uruchamiam z CWD"))
        root = Path.cwd()

    iso  = Path(args.iso)  if args.iso else find_iso(root)
    elf  = Path(args.elf)  if args.elf else find_elf(root)
    bcmd = find_build_cmd(root)

    print(info(f"ISO:  {iso or 'nie znaleziono'}"))
    print(info(f"ELF:  {elf or 'nie znaleziono'}"))
    print(info(f"Build: {' '.join(bcmd) if bcmd else 'nie znaleziono'}"))

    # ── Build ─────────────────────────────────────────────────────
    if not args.no_build and bcmd:
        build_cwd = Path(bcmd[-1]).parent if bcmd[0] == "sh" else root
        if not run_build(bcmd, root):
            print(warn("Build nie powiódł się — próbuję uruchomić stary ISO"))
        else:
            # Odśwież ścieżkę ELF po buildzie
            if not elf:
                elf = find_elf(root)
    elif args.no_build:
        print(info("Pominięto build (--no-build)"))

    # ── Sprawdź ISO ───────────────────────────────────────────────
    if not iso or not iso.exists():
        print(err(f"ISO nie znalezione: {iso}"))
        print(warn("Podaj ścieżkę: python3 crash_analyzer.py --iso path/to/cosinus.iso"))
        sys.exit(1)

    # ── ELF: sprawdź trampoliny przed uruchomieniem ───────────────
    if elf and elf.exists():
        print(f"\n{W}[PRE-CHECK] Weryfikacja ELF przed uruchomieniem{RS}")
        syms = nm_check_tramps(elf)
        if "tramp_u" in syms:
            print(ok(f"tramp_u = {hex(syms['tramp_u'])} znalezione w ELF"))
        else:
            print(err("tramp_u NIE MA W ELF! Trampolina nie zostanie wywołana."))
            print(warn("Napraw build.zig — dodaj tramp.o do komendy ld, potem przebuduj"))
            # Nie przerywaj — uruchom i tak żeby zobaczyć co się stanie
        if "tramp_k" in syms:
            print(ok(f"tramp_k = {hex(syms['tramp_k'])} znalezione w ELF"))

    # ── QEMU ──────────────────────────────────────────────────────
    print(f"\n{W}[QEMU] Uruchamiam maszynę wirtualną...{RS}")
    print(f"{DG}(timeout: {args.timeout}s, Ctrl+C aby przerwać){RS}\n")

    lines, qemu_rc = run_qemu(args.qemu, iso, args.ram, args.timeout)

    print(f"\n{DG}QEMU zakończony (rc={qemu_rc}, {len(lines)} linii logu){RS}")

    # ── Parsuj i raportuj ─────────────────────────────────────────
    st = State()
    for line in lines:
        parse_line(line, st)

    print_report(st, elf)

if __name__ == "__main__":
    main()