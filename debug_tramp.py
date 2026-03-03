#!/usr/bin/env python3
import subprocess, struct, sys

ELF = "build/kernel.elf"

def run(cmd):
    r = subprocess.run(cmd, shell=True, capture_output=True, text=True)
    return r.stdout + r.stderr

# 1. Wszystkie symbole zawierające tramp/TRAMP
print("=" * 60)
print("SYMBOLE tramp/TRAMP w ELF:")
print("=" * 60)
out = run(f"nm {ELF} | grep -i tramp")
print(out if out.strip() else "BRAK JAKICHKOLWIEK SYMBOLI tramp!")

# 2. Wszystkie symbole zawierające TRAMP_U_ADDR / TRAMP_K_ADDR
print("=" * 60)
print("SYMBOLE TRAMP_U_ADDR / TRAMP_K_ADDR:")
print("=" * 60)
out = run(f"nm {ELF} | grep 'TRAMP'")
print(out if out.strip() else "BRAK - global_asm nie wygenerował symboli!")

# 3. Disassembly init_thread_stack
print("=" * 60)
print("DISASSEMBLY init_thread_stack:")
print("=" * 60)
# Znajdź adres
syms = run(f"nm {ELF}")
init_addr = None
for line in syms.splitlines():
    if 'init_thread_stack' in line:
        init_addr = int(line.split()[0], 16)
        print(f"  adres: 0x{init_addr:x}")
        break
if init_addr:
    out = run(f"objdump -d --start-address=0x{init_addr:x} "
              f"--stop-address=0x{init_addr+200:x} {ELF}")
    print(out)
else:
    print("NIE ZNALEZIONO init_thread_stack w symbola!")

# 4. Disassembly tramp_u i tramp_k
print("=" * 60)
print("DISASSEMBLY tramp_u / tramp_k:")
print("=" * 60)
for sym in ['tramp_u', 'tramp_k']:
    addr = None
    for line in syms.splitlines():
        if line.endswith(sym) or f' {sym}' in line:
            addr = int(line.split()[0], 16)
            break
    if addr:
        print(f"  {sym} @ 0x{addr:x}")
        out = run(f"objdump -d --start-address=0x{addr:x} "
                  f"--stop-address=0x{addr+32:x} {ELF}")
        print(out)
    else:
        print(f"  {sym}: NIE MA W ELF (no_mangle nie działa!)")

# 5. Sprawdź sekcję .data pod kątem TRAMP_U_ADDR
print("=" * 60)
print("ZAWARTOSC .data (pierwsze 128 bajtów):")
print("=" * 60)
out = run(f"objdump -s -j .data {ELF} | head -30")
print(out)

# 6. Co dokładnie robi init_thread_stack z adresem trampoliny
print("=" * 60)
print("RAW: jak init_thread_stack ładuje adres trampoliny:")
print("=" * 60)
if init_addr:
    out = run(f"objdump -d --start-address=0x{init_addr:x} "
              f"--stop-address=0x{init_addr+400:x} {ELF} | "
              f"grep -A2 -B2 'mov\\|lea\\|call\\|101'")
    print(out)

# 7. Sprawdź czy TRAMP_U_ADDR istnieje jako dane w ELF
print("=" * 60)
print("SZUKAM wartosci 0x1012xx w sekcji .data (adres tramp_u):")
print("=" * 60)
# Odczytaj plik binarnie i szukaj wzorca
with open(ELF, 'rb') as f:
    data = f.read()

# Znajdź adres tramp_u z nm
tramp_u_addr = None
for line in syms.splitlines():
    if line.strip().endswith(' tramp_u') or ' T tramp_u' in line:
        tramp_u_addr = int(line.split()[0], 16)
        break

if tramp_u_addr:
    print(f"tramp_u powinien być pod: 0x{tramp_u_addr:x}")
    needle = struct.pack('<Q', tramp_u_addr)
    offset = data.find(needle)
    if offset >= 0:
        print(f"ZNALEZIONO adres 0x{tramp_u_addr:x} w pliku @ offset 0x{offset:x}")
        # Sprawdź jaka to sekcja
        out = run(f"objdump -h {ELF}")
        print("Sekcje:")
        print(out)
    else:
        print(f"NIE ZNALEZIONO 0x{tramp_u_addr:x} NIGDZIE w pliku ELF!")
        print("=> global_asm .quad tramp_u NIE ZOSTAŁO WLINKOWANE")
else:
    print("tramp_u nie ma w ELF!")

# 8. Sprawdź Rust source - jak pobierany jest adres
print("=" * 60)
print("RUST SOURCE - init_thread_stack pobieranie adresu:")
print("=" * 60)
try:
    src = open("src/kernel/src/lib.rs").read()
    idx = src.find("fn init_thread_stack")
    if idx >= 0:
        print(src[idx:idx+600])
    else:
        print("NIE ZNALEZIONO init_thread_stack w src!")
except:
    print("Nie mogę otworzyć src/kernel/src/lib.rs")