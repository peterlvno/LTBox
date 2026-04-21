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
- ⚡ **Partition flashing** — read/write individual partitions via EDL (Emergency Download) mode

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

1. Download the [latest release](../../releases/latest) and extract it (no spaces or special chars in path)
2. Double-click **`start.bat`**
3. Follow the on-screen menu

---

## 📋 What Can It Do?

### Main Menu

| Option | What it does |
|---|---|
| **Install firmware (Wipe/Keep)** | All-in-one: convert region → patch → flash. Wipe or keep data. |
| **Disable/Enable System Updates** | Prevent or restore OTA updates via ADB |
| **Rescue from Boot Failure** | Fix boot issues after OTA on a converted device |
| **Root device** | Root with KernelSU / KernelSU Next / SukiSU / ReSukiSU / APatch / FolkPatch |
| **Unroot device** | Restore stock boot image from backup |
| **Settings** | Preset, region, rollback, language, skip ADB |
| **Advanced** | Individual steps — see below |

### Root Providers

**Magisk variants** — classic ramdisk injection

| Provider |
|---|
| Magisk |
| Other Forks |


**KernelSU variants** — LKM (loadable kernel module) or GKI (custom kernel) mode

| Provider | LKM | GKI |
|---|---|---|
| KernelSU | ✅ | ✅ |
| KernelSU Next | ✅ | ✅ |
| SukiSU Ultra | ✅ | ✅ |
| ReSukiSU | ✅ | ✅ |

**APatch variants** — direct boot image patch (GKI)

| Provider |
|---|
| APatch |
| FolkPatch |

> Y700 2nd Gen only supports KernelSU variants in GKI mode and APatch variants.

### Advanced Menu

Step-by-step manual control:

- Convert region (vendor_boot + vbmeta rebuild)
- Dump / patch / flash devinfo & persist
- Detect and patch anti-rollback indices
- Decrypt `.x` files → XML
- Modify XML for flashing (wipe or keep data)
- Flash firmware or selected partitions via EDL
- Rebuild vbmeta for modified images
- Sign & flash custom recovery

---

## 🔧 How It Works (Briefly)

**Region conversion** patches bytes in `vendor_boot.img` (PRC↔ROW region identifiers), then re-signs the image with AOSP test keys and rebuilds `vbmeta.img` so the bootloader accepts it.

**Rooting** unpacks `boot.img` or `init_boot.img`, injects root provider files into the ramdisk (CPIO archive), repacks, and re-signs with the original AVB keys. The device boots the modified image because the bootloader trusts the test key signature.

**Anti-rollback bypass** reads the device's current rollback index via Fastboot, then re-signs the target firmware images with a matching index so the bootloader doesn't reject them as "older" builds.

**All flashing** goes through EDL (Qualcomm Emergency Download) mode — LTBox handles the full flow: ADB → Fastboot → EDL transition, programmer upload, partition read/write, and reset.

---

## 🛠️ Utilities

**`info_image.bat`** — drag and drop `.img` files or folders to view AVB metadata.

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
