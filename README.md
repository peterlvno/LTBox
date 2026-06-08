# LTBox

[🇰🇷 한국어](READMEs/README_ko-KR.md) / [🇨🇳 简体中文](READMEs/README_zh-CN.md)

[![License: CC BY-NC-SA 4.0][cc-by-nc-sa-shield]][cc-by-nc-sa]

## ⚠️ Disclaimer

**Educational purposes only.** Modifying firmware can brick your device, cause data loss, or void your warranty. The author assumes **no liability**. You are solely responsible. **Use at your own risk.**

---

## 🚀 Quick Start

![Windows](https://img.shields.io/badge/Windows-0078D6?logo=windows&logoColor=white) ![Linux](https://img.shields.io/badge/Linux-FCC624?logo=linux&logoColor=black) ![macOS](https://img.shields.io/badge/macOS-000000?logo=apple&logoColor=white)

See **[Quick Start](https://github.com/miner7222/LTBox/wiki/Home#quick-start)** on the wiki.

---

## 📋 What Can It Do?

The app is a sidebar-driven GUI. Each entry opens a guided wizard.

| Sidebar entry | What it does |
|---|---|
| **Dashboard** | Device status, region, recent folders, one-click actions |
| **Flash Firmware** | All-in-one: region → target → wipe/keep → flash. Drives region conversion + rollback handling end-to-end. |
| **System Update** | Disable or enable OTA updates; **Boot Recovery** for rescuing a device that failed to boot after an OTA on a converted region |
| **Root Device** | Root with KernelSU / KernelSU Next / SukiSU / ReSukiSU / APatch / FolkPatch / Magisk (+ forks) |
| **Unroot Device** | Restore the stock boot image from a prior Root backup |
| **Reboot** | Jump to System, Recovery, Bootloader, or EDL |
| **Advanced** | Individual pipeline steps for manual control — see below |
| **Settings** | Language (en/ko/zh/ru), theme (system/light/dark), default EDL loader path |

### Advanced Menu

<details>
<summary>Step-by-step manual control over the pipeline, grouped into three sections</summary>

<br>

**Region & patch**
- Convert region (vendor_boot + vbmeta rebuild)
- Patch devinfo / persist

**Rollback**
- Inspect `.img` AVB metadata
- Detect anti-rollback index
- Patch anti-rollback index
- Rebuild vbmeta for modified images

**EDL ops**
- Decrypt `.x` files → XML
- Dump / flash partitions by name (GPT-by-name, EDL)
- Dump / flash physical LUNs (whole-LUN, EDL)

</details>

---

## 🏗️ Project Layout

| Crate | Role |
|---|---|
| `ltbox-core` | Primitives — errors, settings, logging, GitHub / nightly.link / Lenovo PTSTPD clients, crypto, XML decrypt, live-log sink |
| `ltbox-device` | Transport layer — ADB, Fastboot, EDL / QDL, serialport discovery, Windows Qualcomm USB driver probe + auto-install |
| `ltbox-patch` | Image pipeline — AVB (embedded AOSP testkey specs), boot image ramdisk patching, region conversion, rollback index handling, root provider integration |
| `ltbox-gui` | `iced` desktop app — the `ltbox.exe` binary |

---

## 🙏 Credits

- **Anonymous [ㅇㅇ](https://gall.dcinside.com/board/lists?id=tabletpc)**
- **[갓파더](https://ppomppu.co.kr/zboard/view.php?id=androidtab&page=1&divpage=38&no=197457)**
- **[limzei89](https://note.com/limzei89/n/nd5217eb57827)**
- **[hitin911](https://xdaforums.com/m/hitin911.12861404/)**

---

## 📄 License

This work is licensed under [CC BY-NC-SA 4.0][cc-by-nc-sa].

[![CC BY-NC-SA 4.0][cc-by-nc-sa-image]][cc-by-nc-sa]

[cc-by-nc-sa]: http://creativecommons.org/licenses/by-nc-sa/4.0/
[cc-by-nc-sa-image]: https://licensebuttons.net/l/by-nc-sa/4.0/88x31.png
[cc-by-nc-sa-shield]: https://img.shields.io/badge/License-CC%20BY--NC--SA%204.0-lightgrey.svg
