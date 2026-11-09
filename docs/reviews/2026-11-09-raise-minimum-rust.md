# Review-Protokoll: ci/raise-minimum-rust

Datum: 2026-11-09
Ablage: `docs/reviews/2026-11-09-raise-minimum-rust.md` (Handbuch 29.4)

## Gegenstand

| Feld | Wert |
|------|------|
| Geprüfter Commit | 31e120b1d997aeff4d8d85a8f231990145e6af3c, auf dem Remote-Branch `ci/raise-minimum-rust` erhalten |
| Integrationsbasis | `main` @ 09fadd5e691d7a499e14ad4403a8ec1763749ab7 |
| Umfang | gesamter Diff `origin/main..31e120b`, 20 Dateien: Code, Tests, CI, Doku, Manpage, Prüfprotokolle |
| Kriterien | Regelwerk der Repositories, Handbuch Abschnitte 13 und 33; ein Befund blockiert bei einem Defekt im geänderten Code oder einer falschen oder widersprüchlichen Aussage in geändertem Text |
| Kontextübergabe (Pflicht) | frische Session ohne Autorenkontext; drei aufeinanderfolgende Urteile derselben Prüfinstanz, also keine zusätzliche unabhängige Prüfung |
| Prüfwerkzeug | nicht im Repo geführt (Repo-Hygiene), Ablage: ykush-rs-2026-11-09 |
| Integration | Der Squash-Commit nennt den geprüften Commit und dieses Protokoll (33.4); zum geprüften Stand kommt nur diese Datei hinzu (29.4) |

## Prüfer: Rolle und Art

| Rolle | Art | Name | Grad der Unabhängigkeit |
|-------|-----|------|-------------------------|
| Code-Reviewer | Werkzeug-Review | - | getrennte Session, Eingaben: Auftrag, Diff, Regelwerk, Quellen |

Ein Werkzeug-Review ist eine Vorprüfung, kein Nachweis einer unabhängigen
Bewertung. Die Art sagt nichts über die Kontextübergabe aus.

## Urteil

ACCEPT für den Stand 31e120b, gesamter Diff gegen `origin/main`; kein blockierender Befund
(Reviewer-Ausgabe). Das Urteil autorisiert weder Merge noch Release. Der Squash ist vom
Rechteinhaber am 2026-11-09 beauftragt.

Die beiden vorhergehenden Urteile der Prüfinstanz lauteten CHANGE_REQUIRED für cc39cbe (F1, F2)
und für 402b479 (F3). Für F3 hat das Review selbst festgehalten, dass der Fehler schon in cc39cbe
stand und das erste Urteil ihn nicht erfasst hatte.

## Findings (unverändert aus der Reviewer-Ausgabe)

| ID | Befund | Datei:Zeile | Status | Blockierend |
|---|---|---|---|---|
| F1 | Falsche Aussagen in der Zeile "Parsing the answers" | `SECURITY-REVIEW-0.4.0.md:285` | behoben | nein |
| F2 | Falsche Zuschreibung von "`sysctl` discovery" an `SECURITY.md` | `SECURITY-REVIEW-0.4.0.md:332` | behoben | nein |
| F3 | Falsche Zuschreibung von Zeilen an `SECURITY.md` durch die Tabellenueberschrift | `SECURITY-REVIEW-0.4.0.md:325-327`, `331` | behoben | nein |
| N1 | Die Unterschiedstabelle hat keine Zeile fuer `-g a`; das Original gibt dort `-1` zurueck, ohne die Platine zu fragen (`ykush3.cpp:169-170`). `README.md:138` nennt die Tabelle "the full list", bezogen auf still Toleriertes. Das `CHANGELOG` nennt `-g a`. | `README.md:729-751` | bestaetigt | nein |
| N2 | Unveraenderte Zeile "an invalid port number, help with no explanation": Nach `ykush3` gibt das Original keine Hilfe aus (`port_up` -1, `port_down` 0, `ykush3.cpp:116-118`, `142-144`). Zeile nicht Teil dieser Aenderung. | `README.md:737` | bestaetigt | nein |
| N3 | Die Korrektur von 63 auf 62 ist inhaltlich richtig, steht aber im datierten 0.4.0-Teil. Der Korrekturhinweis in Zeile 247 gilt nur fuer den Nachtrag und erwaehnt sie nicht. | `SECURITY-REVIEW-0.4.0.md:79` | bestaetigt | nein |
| N4 | Testluecke: `-g a` mit Fehler nach dem ersten Port gibt Teilausgabe plus Fehlercode aus. Das ist weder getestet noch dokumentiert. | `src/main.rs:90-94` | plausibel | nein |
| N5 | Oeffnet ein Aufrufer waehrend `hidraw_detach` den Knoten, liefert FreeBSD `ENXIO` (`hidraw.c:354`). `NodeWait` behandelt das als `Failed`, nicht als `Gone`, das Warten bricht also im Reset-Fenster mit 69 ab (vorher 74). Der Abbruch selbst bestand schon vorher, geaendert hat sich nur der Exit-Code. Nicht gemessen. | `src/device.rs:450-458` | plausibel | nein |
| N6 | Fehlt der Platine der Interrupt-OUT-Endpunkt, gibt FreeBSD `ENODEV` zurueck (`usbhid.c:326-327`). Das Programm meldete dann "no longer connected". Ob der Endpunkt existiert, ist `UNKNOWN`; `README.md` und `SECURITY.md` nennen das offen. | `src/error.rs:152` | plausibel | nein |
| N7 | Exit 101 passt nicht zur Formulierung "follows sysexits(3)" in der Manpage und liegt ausserhalb von "64 to 77" im Hilfetext. | `man/ykushctrl.1:189`, `src/help.rs:72` | bestaetigt | nein |
| N8 | `MISSING_SOURCE`: VID/PID in der Manpage stammen aus dem Original (`ykush3.h:59`), die I2C-Ausrichtung aus dem Bestands-Hilfetext und dem Hilfetext des Originals (LEGACY). Ein Datenblatt lag fuer dieses Review nicht vor. | `man/ykushctrl.1:15-17`, `man/ykushctrl.1:122-126` | bestaetigt | nein |
| N9 | Abschnitt 12: Das Repository hat keine Requirements-Baseline und kein Detailed Design (`MISSING_SOURCE`). Geprueft wurde deshalb gegen README, SECURITY, CHANGELOG und Manpage. Die Tests sind weder als Entwicklertests gekennzeichnet noch von einer getrennten Rolle erstellt, also kein unabhaengiger Nachweis. Das betrifft auch `sanitize` und `is_hidraw_name`. | gesamtes Repository | bestaetigt | nein |
| N10 | `hidraw_boards` laeuft in keinem CI-Test, FreeBSD wird dort nur per `check` und `clippy` geprueft. Das ist in `ci.yml` korrekt beschrieben. | `src/device.rs:599-633` | bestaetigt | nein |
| N11 | CI meldet die Deprecation von Node.js 20 fuer die gepinnten Actions. | `.github/workflows/*.yml` | bestaetigt | nein |
| N12 | Neu gefasst, der Teil zur Zuschreibung an `SECURITY.md` ist durch F3 erledigt. Zeile 329 ist allgemeiner als der Code: Schalt- und Konfigurationsbefehle pruefen nur das Statusbyte, nicht den Bezug zur Anfrage. Zeile 285 und B-ykush-3 im selben Abschnitt sagen das korrekt, daher kein Widerspruch im Dokument. | `SECURITY-REVIEW-0.4.0.md:329` | bestaetigt | nein |
| N13 | Die Bewertung in Zeile 330 verweist nur auf `TIMEOUT` und `Link`. Die Kernelgrenze ist dort nicht zu sehen, sie folgt erst aus den Kernelquellen. Die Aussage selbst stimmt. | `SECURITY-REVIEW-0.4.0.md:330` | bestaetigt | nein |
| N14 | Die Commit-Message von `31e120b` sagt, alle Zeilen seien gegen den Code geprueft; Zeile 333 ist gegen `SECURITY.md` geprueft. Der Squash ersetzt die Nachricht. | Commit `31e120b` | bestaetigt | nein |

Offene Findings sind nicht entschieden: Die Einstufung "nicht blockierend" ist die des Reviews,
Entscheidung der zuständigen Rolle offen.

## Gate-Ergebnisse

Exit-Codes aus dem tatsächlichen Lauf. Am Endstand 31e120b liefen:

| Gate | Kommando | Exit-Code |
|------|----------|-----------|
| Tests | `cargo test --all-features --locked` (177 und 1 Doctest, 11 ignoriert) | 0 |
| Text-Gate | `check_text.py --root <worktree> --commits origin/main..31e120b` (0 Findings) | 0 |
| CI | Lauf 42654511283, Stand 31e120b, alle 6 Jobs: `test`, `lint`, `minimum-supported-rust` auf `macos-latest` und `ubuntu-24.04-arm` | success |

Am Stand cc39cbe liefen zusätzlich; seither ändert sich nur `SECURITY-REVIEW-0.4.0.md`:

| Gate | Kommando | Exit-Code |
|------|----------|-----------|
| Lint | `cargo clippy --all-targets --all-features --locked -- -D warnings` | 0 |
| Lint Linux | `cargo clippy --all-targets --locked --target aarch64-unknown-linux-gnu -- -D warnings` | 0 |
| MSRV | `cargo +1.98 check --all-targets --locked` | 0 |
| MSRV FreeBSD | `cargo +1.98 check --all-targets --locked --target aarch64-unknown-freebsd` | 0 |
| Lint FreeBSD | `cargo +1.98 clippy --all-targets --locked --target aarch64-unknown-freebsd -- -D warnings` | 0 |
| Format | `cargo fmt --check` | 0 |
| Lieferkette | `cargo deny --all-features check` | 0 |
| Manpage | `mandoc -T lint -W warning man/ykushctrl.1` | 0 |
| Abdeckung | `cargo test --locked` instrumentiert, ohne `--ignored` | 0 |
| Dependency Audit | Lauf 42645306215 | success |
| Hardware | `cargo test --locked --no-fail-fast -- --ignored --test-threads=1`, macOS, Board `Y3N13808`, Firmware 1.5.0: 11 bestanden (9 Board, 2 HID-Stack), 0 fehlgeschlagen; Ports 1 bis 4 davor und danach aus | 0 |

## Nicht verifiziert

| Punkt | Markierung |
|---|---|
| Hardware an 31e120b: gelaufen an cc39cbe; seither keine Codeänderung | Grenze |
| Linux und FreeBSD am Board auf diesem Stand: nicht gelaufen, Unplug-Pfade und Schreibgrenzen nur aus dem Quelltext; FreeBSD-Tests laufen auch in CI nicht | `UNKNOWN` |
| `-g a` und geschlossene Pipe an einem echten Board: `-g a` nur lesend über das Programm, nicht als Test | `UNKNOWN` |
| Interrupt-OUT-Endpunkt des Boards | `UNKNOWN` |
| Datenblatt-Tabellen zu Protokollwerten | `MISSING_SOURCE` |
| Rote TDD-Phase: keine Laufnachweise | Grenze |

## Abweichungen vom Ablauf

- Drei Commits des Branches entstanden am 2026-11-07 direkt auf dem lokalen `main`, ohne
  vorheriges Review (G3). Sie waren nie auf dem Remote; `main` wurde am 2026-11-09 auf
  `origin/main` zurückgesetzt und die Commits auf dem Branch weitergeführt.
- Der Branch wurde zunächst im Hauptverzeichnis bearbeitet statt in einem Worktree (Handbuch
  33.2 Nr. 6). Mit Zustimmung des Rechteinhabers ging es am 2026-11-09 zurück auf `main`, der
  Rest entstand im Worktree.
- In einem früheren Review dieses Branches wurde gegen den Auftrag ein Aufruf des Programms mit
  `-u 1` ausgeführt; die Ausgabe ging verloren. Ob er ein Board erreichte, ist `UNKNOWN`. Port 1
  stand vor dem Hardware-Lauf auf `off`.
- In einem früheren Review wurde nach einem abgelehnten Vergleichsbefehl derselbe Vergleich mit
  einem anderen Werkzeug ausgeführt (G7). Welcher Vergleich das war, ist nicht festgehalten.

## Offene Punkte vor einem Release

1. Hardware-Tests und Abziehen im Betrieb auf Linux (Raspberry Pi) und FreeBSD an den
   aktuellen Ständen, darunter `ENXIO`, `ENODEV` und `--reset`/`--boot` mit Exit 0 ohne
   Zustellung.
2. M1-Versuch (verwaiste Antwort) auf Linux.
3. Lokaler und entfernter Tag `v0.4.0` zeigen auf verschiedene Commits (1e503fa, 69137b6).

## Altlasten außerhalb dieser Änderung

Requirements und Trennung der Tests (N9), `ENXIO` beim Warten auf FreeBSD (N5), die weiteren
Findings N1 bis N4, N6 bis N8 und N10 bis N13 sowie nicht auflösbare Hashes in älteren Protokollen
(`6a4dca9`, `e0aec7b`). Lokale Anweisungsdateien und `worktrees/` stehen nur in
`.git/info/exclude`, nicht in `.gitignore`, und `SECURITY-REVIEW.md:35` nennt weiter "Highest
index 63".
