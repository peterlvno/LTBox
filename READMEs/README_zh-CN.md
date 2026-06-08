# LTBox

[🇺🇸 English](../README.md) / [🇰🇷 한국어](README_ko-KR.md)

[![License: CC BY-NC-SA 4.0][cc-by-nc-sa-shield]][cc-by-nc-sa]

## ⚠️ 免责声明

**仅供教育用途。** 修改固件可能导致设备变砖、数据丢失或保修失效。作者**不承担任何责任**。所有后果由用户自行承担。**使用风险自负。**

---

## 🚀 快速开始

![Windows](https://img.shields.io/badge/Windows-0078D6?logo=windows&logoColor=white) ![Linux](https://img.shields.io/badge/Linux-FCC624?logo=linux&logoColor=black) ![macOS](https://img.shields.io/badge/macOS-000000?logo=apple&logoColor=white)

请参阅 wiki 上的 **[快速开始](https://github.com/miner7222/LTBox/wiki/Home#quick-start)**。

---

## 📋 功能介绍

侧边栏驱动的 GUI，每个入口打开一个引导式向导。

| 侧边栏条目 | 说明 |
|---|---|
| **仪表盘** | 设备状态、区域、最近使用的文件夹、一键操作 |
| **固件刷写** | 一键操作：区域 → 目标 → 清除/保留 → 刷写。端到端处理区域转换和回滚 |
| **系统更新** | 禁用或启用 OTA 更新；**启动恢复**用于抢救区域转换后 OTA 导致启动失败的设备 |
| **Root 设备** | 使用 KernelSU / KernelSU Next / SukiSU / ReSukiSU / APatch / FolkPatch / Magisk（及分支）获取 Root |
| **取消 Root** | 从之前的 Root 备份恢复原始引导镜像 |
| **重启** | 跳转到系统 / Recovery / Bootloader / EDL |
| **高级菜单** | 单独的流水线步骤供手动控制 — 见下方 |
| **设置** | 语言（en/ko/zh/ru）、主题（系统/浅色/深色）、默认 EDL 加载器路径 |

### 高级菜单

<details>
<summary>逐步手动控制流水线，分为三个部分</summary>

<br>

**区域 & 补丁**
- 区域转换（vendor_boot + vbmeta 重建）
- 补丁 devinfo / persist

**回滚**
- 查看 `.img` AVB 元数据
- 检测反回滚索引
- 补丁反回滚索引
- 为修改后的镜像重建 vbmeta

**EDL 操作**
- 解密 `.x` 文件 → XML
- 按名称转储 / 刷写分区（GPT-by-name, EDL）
- 物理 LUN 整体转储 / 刷写（whole-LUN, EDL）

</details>

---

## 🏗️ 项目结构

| Crate | 职责 |
|---|---|
| `ltbox-core` | 基础原语 — 错误、设置、日志、GitHub / nightly.link / 联想 PTSTPD 客户端、加密、XML 解密、实时日志接收器 |
| `ltbox-device` | 传输层 — ADB、Fastboot、EDL / QDL、serialport 探测、Windows 高通 USB 驱动检测 + 自动安装 |
| `ltbox-patch` | 镜像流水线 — AVB（内嵌 AOSP testkey 规范）、引导镜像 ramdisk 补丁、区域转换、回滚索引处理、Root 方案集成 |
| `ltbox-gui` | `iced` 桌面应用 — `ltbox.exe` 二进制 |

---

## 🙏 致谢

- **Anonymous [ㅇㅇ](https://gall.dcinside.com/board/lists?id=tabletpc)**
- **[갓파더](https://ppomppu.co.kr/zboard/view.php?id=androidtab&page=1&divpage=38&no=197457)**
- **[limzei89](https://note.com/limzei89/n/nd5217eb57827)**
- **[hitin911](https://xdaforums.com/m/hitin911.12861404/)**

---

## 📄 许可证

本作品基于 [CC BY-NC-SA 4.0][cc-by-nc-sa] 许可证发布。

[![CC BY-NC-SA 4.0][cc-by-nc-sa-image]][cc-by-nc-sa]

[cc-by-nc-sa]: http://creativecommons.org/licenses/by-nc-sa/4.0/
[cc-by-nc-sa-image]: https://licensebuttons.net/l/by-nc-sa/4.0/88x31.png
[cc-by-nc-sa-shield]: https://img.shields.io/badge/License-CC%20BY--NC--SA%204.0-lightgrey.svg
