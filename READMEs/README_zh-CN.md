# LTBox

[English](../README.md)

[![License: CC BY-NC-SA 4.0][cc-by-nc-sa-shield]][cc-by-nc-sa]

## ⚠️ 重要：免责声明

**本项目仅用于「教育」目的。**

修改设备固件会带来重大风险，包括但不限于：设备变砖、数据丢失或保修失效。**作者不承担任何责任**，亦**不**对因使用这些脚本而可能**对您的设备或任何其他人的设备**造成的**任何损害或后果**负责。

**您须自行承担一切后果。使用本程序，风险自负。**

---

## 1. 核心漏洞 & 概述

该工具包利用了某些联想 Android 平板电脑中存在的安全漏洞。这些设备的固件使用了公开的 AOSP（Android Open Source Project）测试密钥进行签名。

由于这种漏洞，即使 bootloader 被锁定 ，设备的 bootloader 也会信任并启动任何使用这些常用测试密钥签名的映像。

该工具包是一个包含多种脚本的集合，利用此漏洞，对**已锁定 bootloader 的设备**执行高级修改。

### 目标型号

* 联想拯救者 Y700 2nd, 3rd, 4th Gen (又名 Legion Tab)
* 联想 Yoga Pad Pro AI (又名 Yoga Tab Plus AI)
* 联想小新 Pad Pro GT (又名 Yoga Tab 11.1 AI)
* *...其他近期发布的联想设备（2023 年或之后发布，采用高通芯片组）也可能存在漏洞。*

## 2. 使用方法

该工具包设计为**完全自动化**。

1.  **下载并解压：** 下载最新版本并将其解压到文件夹（确保路径中不包含**空格或非 ASCII 字符**）。
2.  **运行脚本：** 双击 `start.bat` 。
    * *首次运行时将自动安装依赖项。*
3.  **选择任务：** 按照屏幕菜单选择您想要执行的操作。

## 3. 对菜单的描述

### 3.1 主菜单

**`1. 在 PRC/ROW 设备安装固件 [擦除数据]`**
一体化自动化任务。它执行所有步骤（转换、XML 准备、转储、修补、ARB 检查、刷写），并**清除所有用户数据**。（菜单文本根据选定的目标区域而变化）。

**`2. 在 PRC/ROW 设备安装固件 [保留数据]`**
与选项 1 相同，但修改 XML 脚本以**保留用户数据** （跳过 `userdata` 和 `metadata` 分区）。

**`3. OTA 更新后救砖`**
尝试通过转储和修补重要分区来修复因在转换设备上进行完整 OTA 更新而导致的启动问题。

**`4. 禁用 OTA`**
以 ADB 模式连接到设备，并禁用系统更新包以防止自动更新。

**`5. Root 设备`**
打开 root 提权方式选择菜单：
* **LKM Mode:** 修补 `init_boot.img` 和 `vbmeta.img` （推荐用于较新的内核）。支持 KernelSU Next、SukiSU Ultra、ReSukiSU 和 FolkPatch。
* **GKI Mode:** 通过将 `boot.img` 的内核替换为 `GKI_KernelSU_SUSFS` 来修补 `boot.img` 。

**`6. 设备 Unroot`**
通过从备份中刷入官方镜像，将设备恢复到未 root 状态。

**`0. 设置`**
打开设置子菜单以配置工具包（见下文）。

**`a. 高级菜单`**
打开高级菜单以进行手动控制和故障排除。

### 3.2 设置菜单

* **Region:** 在 PRC （中国）和 ROW （全球）之间切换目标固件区域。
* **Skip ADB:** 跳过 ADB 检查。如果设备已处于 EDL/Fastboot 模式，则此功能很有用。
* **Skip Anti-Rollback:** 跳过自动防回滚检查。
* **Language:** 切换工具包的界面语言。
* **Check for Updates:** 检查 LTBox 的最新版本。

### 3.3 高级菜单

用于手动控制和故障排除的各个步骤。

**`1. 为中国版 (PRC) / 全球版 (ROW) 设备更改固件区域`**
根据选定的区域设置（PRC 或 ROW）转换 `vendor_boot.img` 并重建 `vbmeta.img` 。

**`2. 从设备导出 devinfo/persist`**
将 EDL 模式下设备的 `devinfo` 和 `persist` 分区转储到 `backup/` 文件夹。

**`3. 修补 devinfo/persist 以更改国家代码`**
修补 `devinfo.img` / `persist.img` 中的国家代码（例如，`CN`、`KR`）。

**`4. 将 devinfo/persist 写入设备`**
通过 EDL 将修补后的镜像刷写到设备。

**`5. 从设备检测防回滚状态`**
转储 `boot` 和 `vbmeta_system` ，以检查它们的回滚索引是否与新的 ROM 一致。

**`6. 在 ROM 中修补回滚索引`**
将新 ROM 的回滚索引与设备的索引同步，以绕过防回滚保护。

**`7. 将防回滚修补后的镜像写入设备`**
将经过 ARB 修补的镜像文件刷入设备。

**`8. 将 X 文件转换为 XML`**
将 `.x` （加密）固件文件解密为 `.xml` 文件。

**`9. 修改 XML 用于刷机 [擦除数据]`**
生成 `rawprogram` XML 文件，然后刷入已修补的镜像并**清除用户数据**。

**`10. 修改 XML 用于刷机 [保留数据]`**
与步骤 9 相同，但修改 XML 以**保留用户数据**。

**`11. 刷入固件到设备`**
手动完整刷机。复制所有已修补的文件，并使用 `fh_loader` 刷入。

**`12. 刷入选定分区`**
将选定的分区刷入设备。

**`13. 签名并刷入第三方 Recovery`**
使用测试密钥对自定义恢复映像（例如 TWRP）进行签名，并将其刷入恢复分区。

## 4. 其他实用工具

**`info_image.bat`**
将 `.img` 文件或文件夹拖放到此脚本上，即可使用 `avbtool` 提取详细的图像信息。

## 5. 致谢

特别感谢以下社区成员的贡献和研究：

* **Anonymous [ㅇㅇ](https://gall.dcinside.com/board/lists?id=tabletpc)**
* **[갓파더](https://ppomppu.co.kr/zboard/view.php?id=androidtab&page=1&divpage=38&no=197457)**
* **[limzei89](https://note.com/limzei89/n/nd5217eb57827)**
* **[hitin911](https://xdaforums.com/m/hitin911.12861404/)**

---

## 许可

本作品采用以下许可协议：
[Creative Commons Attribution-NonCommercial-ShareAlike 4.0 International License][cc-by-nc-sa].

[![CC BY-NC-SA 4.0][cc-by-nc-sa-image]][cc-by-nc-sa]

[cc-by-nc-sa]: http://creativecommons.org/licenses/by-nc-sa/4.0/
[cc-by-nc-sa-image]: https://licensebuttons.net/l/by-nc-sa/4.0/88x31.png
[cc-by-nc-sa-shield]: https://img.shields.io/badge/License-CC%20BY--NC--SA%204.0-lightgrey.svg
