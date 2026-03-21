# LTBox

[🇰🇷 한국어](READMEs/README_ko-KR.md) / [🇨🇳 简体中文](READMEs/README_zh-CN.md)

[![License: CC BY-NC-SA 4.0][cc-by-nc-sa-shield]][cc-by-nc-sa]

## ⚠️ Important: Disclaimer

**This project is for educational purposes ONLY.**

Modifying your device's firmware carries significant risks, including but not limited to, bricking your device, data loss, or voiding your warranty. The author **assumes no liability** and is not responsible for any **damage or consequence** that may occur to **your device or anyone else's device** from using these scripts.

**You are solely responsible for any consequences. Use at your own absolute risk.**

---

## 1. Core Vulnerability & Overview

This toolkit exploits a security vulnerability found in certain Lenovo Android tablets. These devices have firmware signed with publicly available **AOSP (Android Open Source Project) test keys**.

Because of this vulnerability, the device's bootloader trusts and boots any image signed with these common test keys, even if the bootloader is **locked**. This allows advanced modifications on a device with a locked bootloader.

### Target Models

* Lenovo Legion Y700 2nd, 3rd, 4th Gen (aka Legion Tab)
* Lenovo Yoga Pad Pro AI (aka Yoga Tab Plus AI)
* Lenovo Xiaoxin Pad Pro GT (aka Yoga Tab 11.1 AI)
* *...Other recent Lenovo devices (released between 2023 and before 2026 with Qualcomm chipsets) may also be vulnerable.*

**Note: Starting from the Legion Tab Y700 5th Gen released in 2026, this vulnerability has been patched, so this tool cannot be used.**

## 2. How to Use

1.  **Download & Extract:** Download the latest release and extract it to a folder (ensure the path contains no spaces or non-ASCII characters).
2.  **Run the Script:** Double-click **`start.bat`**.
    * *Dependencies will be installed automatically on the first run.*
3.  **Select Task:** Follow the on-screen menu to choose your desired operation.

## 3. Menu Descriptions

### 3.1 Main Menu

**`1. Install firmware to PRC/ROW device [WIPE DATA]`**
The all-in-one automated task. It performs all steps (Convert, XML Prepare, Dump, Patch, ARB Check, Flash) and **wipes all user data**. (Menu text changes based on the selected target region).

**`2. Install firmware to PRC/ROW device [KEEP DATA]`**
Same as option 1, but modifies the XML scripts to **preserve user data** (skips `userdata` and `metadata` partitions).

**`3. Rescue after OTA`**
Attempts to fix boot issues caused by taking a Full OTA update on a converted device by dumping & patching essential partitions.

**`4. Disable OTA`**
Connects to the device in ADB mode and disables system update packages to prevent automatic updates.

**`5. Root device`**
Opens the root selection menu. Available root types:
* **KernelSU / KernelSU Next** — LKM & GKI mode
* **SukiSU Ultra / ReSukiSU** — LKM mode
* **APatch / FolkPatch**

Currently, rooting on the Legion Tab Y700 2nd Gen is only possible using KernelSU Next GKI mode and APatch / FolkPatch.

**`6. Unroot device`**
Restores the device to a non-rooted state by flashing the stock image from backups.

**`0. Settings`**
Opens the settings submenu to configure the toolkit (see below).

**`a. Advanced Menu`**
Opens the advanced menu for individual steps, manual control, and troubleshooting.

### 3.2 Settings Menu

* **Preset:** Cycle through device presets (ROW, PRC, Stock). Each preset configures the target region and related defaults.
* **Modify Region Code:** Toggle region code modification ON/OFF. When OFF, firmware is flashed without modifying the region.
* **Region:** Toggle target firmware region between **PRC** (China) and **ROW** (Global). Only visible when Modify Region Code is ON.
* **Skip ADB:** Toggle ADB checks. Useful if the device is already in EDL/fastboot mode.
* **Skip Anti-Rollback:** Toggle automated Anti-Rollback checks.
* **Language:** Switch the toolkit's interface language.
* **Check for Updates:** Check for the latest version of LTBox.

### 3.3 Advanced Menu

Individual steps for manual control and troubleshooting.

**`1. Convert ROM Region to PRC/ROW`**
Converts `vendor_boot.img` and rebuilds `vbmeta.img` with updated verification metadata based on the selected region settings (PRC or ROW). Only visible when Modify Region Code is ON.

**`2. Dump devinfo/persist from device`**
Dumps `devinfo` and `persist` partitions from the device in EDL mode to the `backup/` folder.

**`3. Patch devinfo/persist`**
Patches the country code (e.g., "CN", "KR") in `devinfo.img`/`persist.img`.

**`4. Write devinfo/persist to device`**
Flashes the patched images to the device via EDL.

**`5. Detect Anti-Rollback from device`**
Dumps `boot` and `vbmeta_system` to analyze their AVB rollback indices (security versions) against the new ROM.

**`6. Patch rollback indices in ROM`**
Synchronizes the new ROM's AVB rollback index with the device's index to bypass anti-rollback protection.

**`7. Write Anti-Anti-Rollback patched image`**
Flashes the ARB-patched images to the device.

**`8. Convert X files to XML`**
Decrypts `.x` (encrypted) firmware files into `.xml` files.

**`9. Modify XML for Flashing [WIPE DATA]`**
Generates `rawprogram` XMLs to allow flashing patched images and **wipes user data**.

**`10. Modify XML for Flashing [KEEP DATA]`**
Same as Step 9, but modifies XMLs to **preserve user data**.

**`11. Flash firmware to device`**
Manual full flash. Copies all patched files and flashes them using `fh_loader`.

**`12. Flash selected partitions`**
Flashes selected partitions to the device.

**`13. Rebuild vbmeta for modified images`**
Rebuilds `vbmeta.img` to include updated hash/chain descriptors for any modified partition images (e.g., after manual patching).

**`14. Sign & Flash Custom Recovery`**
Signs a custom recovery image (e.g., TWRP) with test keys and flashes it to the recovery partition.

## 4. Other Utilities

**`info_image.bat`**
Drag and drop `.img` files or folders onto this script to extract detailed image information using avbtool.

## 5. Credits

Special thanks to the following community members for their contributions and research:

* **Anonymous [ㅇㅇ](https://gall.dcinside.com/board/lists?id=tabletpc)**
* **[갓파더](https://ppomppu.co.kr/zboard/view.php?id=androidtab&page=1&divpage=38&no=197457)**
* **[limzei89](https://note.com/limzei89/n/nd5217eb57827)**
* **[hitin911](https://xdaforums.com/m/hitin911.12861404/)**

---

## License

This work is licensed under a
[Creative Commons Attribution-NonCommercial-ShareAlike 4.0 International License][cc-by-nc-sa].

[![CC BY-NC-SA 4.0][cc-by-nc-sa-image]][cc-by-nc-sa]

[cc-by-nc-sa]: http://creativecommons.org/licenses/by-nc-sa/4.0/
[cc-by-nc-sa-image]: https://licensebuttons.net/l/by-nc-sa/4.0/88x31.png
[cc-by-nc-sa-shield]: https://img.shields.io/badge/License-CC%20BY--NC--SA%204.0-lightgrey.svg
