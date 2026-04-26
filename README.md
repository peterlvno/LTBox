# LTBox

[🇰🇷 한국어](READMEs/README_ko-KR.md) / [🇨🇳 简体中文](READMEs/README_zh-CN.md)

[![License: CC BY-NC-SA 4.0][cc-by-nc-sa-shield]][cc-by-nc-sa]

## ⚠️ Disclaimer

**Educational purposes only.** Modifying firmware can brick your device, cause data loss, or void your warranty. The author assumes **no liability**. You are solely responsible. **Use at your own risk.**

---

## 🔑 What Is This?

Certain Lenovo tablets ship with firmware signed using publicly available AOSP test keys. Because of this, the bootloader trusts and boots any image signed with those keys — even when **locked**.

LTBox exploits this to enable:

- 🌍 **Region conversion** — switch between PRC (China) and ROW (Global) firmware
- 🔓 **Root** — install Magisk, KernelSU, APatch, and more on a locked bootloader
- 🛡️ **Anti-rollback bypass** — flash older/newer firmware without rollback protection blocking you
- ⚡ **Partition flashing** — read/write partitions via EDL (Emergency Download) mode

### Supported Devices

| Device | Notes |
|---|---|
| Legion Tab Y700 2nd, 3rd Gen | Full support |
| Legion Tab Y700 4th Gen | ZUXOS ≤ 1.5.10.138 |
| Yoga Pad Pro AI / Yoga Tab Plus AI | Full support |
| Xiaoxin Pad Pro GT / Yoga Tab 11.1 AI | Full support |

> **Note:** Devices released in 2026+ (e.g. Y700 5th Gen) have this vulnerability patched.

---

## 🚀 Quick Start

### Windows

1. Download the [latest release](../../releases/latest) and extract the zip (no spaces or special chars in the path)
2. Double-click **`ltbox.exe`**
3. Pick a task from the sidebar and follow the wizard

Windows `x86_64` and `arm64` builds are published.

### Linux

1. Install runtime deps (Debian/Ubuntu shown — adapt for your distro):
   ```bash
   sudo apt install \
     libusb-1.0-0 libudev1 \
     libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 \
     libxcb1 libxcb-render0 libxcb-shape0 libxcb-xfixes0 \
     libfontconfig1 \
     xdg-utils
   ```
2. Download the [latest release](../../releases/latest) Linux tarball (`tar -xzf LTBox-linux_*.tar.gz`). The executable bit on `ltbox` is preserved.
3. Install the udev rules so the desktop session can open the Qualcomm 9008 / Lenovo USB devices without root:
   ```bash
   sudo ./ltbox --install-udev
   ```
4. **Replug** any connected device.
5. Run `./ltbox`.

Linux `x86_64` and `aarch64` builds are published.

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
| **Settings** | Language (en/ko/zh/ru), theme, default region, rollback preset, ADB skip |

### Advanced Menu

Step-by-step manual control over the pipeline:

- Convert region (vendor_boot + vbmeta rebuild)
- Dump / patch / flash devinfo & persist
- Detect and patch anti-rollback indices
- Decrypt `.x` files → XML
- Modify XML for flashing (wipe or keep data)
- Flash firmware or selected partitions via EDL
- Rebuild vbmeta for modified images
- Sign & flash custom recovery
- Inspect `.img` AVB metadata

---

## 🔧 How It Works (Briefly)

**Region conversion** patches bytes in `vendor_boot.img` (PRC↔ROW region identifiers), then re-signs the image with AOSP test keys and rebuilds `vbmeta.img` so the bootloader accepts it.

**Rooting** unpacks `boot.img` or `init_boot.img`, injects root provider files into the ramdisk, repacks, and re-signs with the original AVB keys. The device boots the modified image because the bootloader trusts the test key signature.

**Anti-rollback bypass** reads the device's current rollback index via Fastboot, then re-signs the target firmware images with a matching index so the bootloader doesn't reject them as "older" builds.

**All flashing** goes through EDL mode — LTBox handles the full flow: ADB → Fastboot → EDL transition, programmer upload, partition read/write, and reset.

---

## 🏗️ Project Layout

| Crate | Role |
|---|---|
| `ltbox-core` | Primitives — errors, settings, logging, GitHub/nightly.link clients, crypto, XML decrypt |
| `ltbox-device` | Transport layer — ADB, Fastboot, EDL / QDL, serialport discovery |
| `ltbox-patch` | Image pipeline — AVB, boot image ramdisk patching, region conversion, rollback, root provider integration |
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
