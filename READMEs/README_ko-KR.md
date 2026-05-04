# LTBox

[🇺🇸 English](../README.md) / [🇨🇳 简体中文](README_zh-CN.md)

[![License: CC BY-NC-SA 4.0][cc-by-nc-sa-shield]][cc-by-nc-sa]

## ⚠️ 면책 조항

**교육 목적으로만 사용하세요.** 펌웨어 수정은 벽돌, 데이터 손실, 보증 무효화 등의 위험이 있습니다. 제작자는 **어떠한 책임도 지지 않습니다**. 모든 결과는 사용자 본인의 책임입니다. **본인의 위험 부담 하에 사용하세요.**

---

## 🔑 이게 뭔가요?

일부 레노버 태블릿은 공개된 AOSP 테스트 키로 서명된 펌웨어를 탑재하고 있습니다. 이로 인해 부트로더가 **잠겨 있어도** 해당 키로 서명된 이미지를 신뢰하고 부팅에 성공합니다.

LTBox는 이를 활용하여 다음을 가능하게 합니다:

- 🌍 **지역 변환** — PRC(중국)↔ROW(글로벌) 펌웨어 전환
- 🔓 **루팅** — 잠긴 부트로더에서 Magisk, KernelSU, APatch 등 설치
- 🛡️ **안티롤백 우회** — 롤백 보호를 우회하여 이전/이후 펌웨어 플래싱
- ⚡ **파티션 플래싱** — EDL(Emergency Download) 모드를 통한 파티션 읽기/쓰기

### 지원 기기

| 기기 | 비고 |
|---|---|
| Legion Tab Y700 2세대, 3세대 | 전체 지원 |
| Legion Tab Y700 4세대 | ZUXOS ≤ 1.5.10.138 |
| Yoga Pad Pro AI / Yoga Tab Plus AI | 전체 지원 |
| Xiaoxin Pad Pro GT / Yoga Tab 11.1 AI | 전체 지원 |

> **참고:** 2026년 이후 출시된 기기(예: Y700 5세대)들에서는 이 취약점이 패치되었습니다.

---

## 🚀 빠른 시작

### Windows

1. [최신 릴리즈](../../releases/latest) 다운로드 후 압축 해제 (경로에 공백/특수문자 없이)
2. **`ltbox.exe`** 더블클릭
3. 사이드바에서 작업을 선택하고 위저드를 따라 진행

Windows `x86_64` 및 `arm64` 빌드가 배포됩니다.

> **퀄컴 USB 드라이버:** 퀄컴 USB 드라이버가 누락된 경우 대시보드에 "드라이버 설치" 배너가 표시됩니다. 클릭하면 GitHub에서 최신 `qcom-usb-kernel-drivers` 릴리즈를 다운로드하여 `pnputil`로 설치합니다. 첫 실행 시 `pnputil`이 `.inf` 파일을 설치할 수 있도록 LTBox를 관리자 권한으로 실행하세요.

### Linux

1. 런타임 의존성 설치 (Debian/Ubuntu 기준 — 다른 배포판은 적절히 변경):
   ```bash
   sudo apt install \
     libusb-1.0-0 libudev1 \
     libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 \
     libxcb1 libxcb-render0 libxcb-shape0 libxcb-xfixes0 \
     libfontconfig1 \
     xdg-utils
   ```
2. [최신 릴리즈](../../releases/latest)의 Linux 타르볼 다운로드 (`tar -xzf LTBox-linux_*.tar.gz`). `ltbox` 실행 비트는 보존됩니다.
3. 데스크톱 세션이 root 없이 Qualcomm 9008 / Lenovo USB 장치를 열 수 있도록 udev 규칙 설치:
   ```bash
   sudo ./ltbox --install-udev
   ```
4. 연결된 기기를 **재연결**하세요.
5. (선택) 사용자 단위 앱 메뉴 항목 + 아이콘 추가 (root 불필요):
   ```bash
   ./ltbox --install-desktop
   ```
   `~/.local/share/applications/`에 `.desktop` 파일을, `~/.local/share/icons/hicolor/scalable/apps/`에 SVG 아이콘을 설치합니다. GNOME / KDE에서 몇 초 내로 인식됩니다. 바이너리를 옮긴 뒤에는 다시 실행하세요.
6. `./ltbox` 실행.

Linux `x86_64` 및 `aarch64` 빌드가 배포됩니다.

---

## 📋 무엇을 할 수 있나요?

사이드바 기반 GUI로 각 항목이 가이드형 위저드를 엽니다.

| 사이드바 항목 | 설명 |
|---|---|
| **대시보드** | 기기 상태, 지역, 최근 폴더, 원클릭 작업 |
| **펌웨어 플래싱** | 올인원: 지역 → 대상 → 초기화/유지 → 플래싱. 지역 변환과 롤백 처리를 엔드투엔드로 수행 |
| **시스템 업데이트** | OTA 업데이트 비활성화/활성화; **부팅 복구**로 지역 변환된 기기의 OTA 후 부팅 실패 복구 |
| **루팅** | KernelSU / KernelSU Next / SukiSU / ReSukiSU / APatch / FolkPatch / Magisk(+포크)로 루팅 |
| **루팅 해제** | 이전 루팅 백업에서 순정 부트 이미지 복원 |
| **재부팅** | System / Recovery / Bootloader / EDL로 이동 |
| **고급 메뉴** | 파이프라인 개별 단계 수동 제어 — 아래 참조 |
| **설정** | 언어(en/ko/zh/ru), 테마(시스템/라이트/다크), 기본 EDL 로더 경로 |

### 고급 메뉴

파이프라인 개별 단계 수동 제어, 세 섹션으로 구성:

**지역 & 패치**
- 지역 변환 (vendor_boot + vbmeta 재구성)
- devinfo / persist 패치

**롤백**
- `.img` AVB 메타데이터 확인
- 안티롤백 인덱스 감지
- 안티롤백 인덱스 패치
- 수정된 이미지에 대한 vbmeta 재구성

**EDL 작업**
- `.x` 파일 복호화 → XML
- 파티션 이름 기준 덤프 / 플래싱 (GPT-by-name, EDL)
- 물리 LUN 단위 덤프 / 플래싱 (전체 LUN, EDL)

---

## 🔧 작동 원리 (간략)

**지역 변환**은 `vendor_boot.img`의 바이트를 패치(PRC↔ROW 지역 식별자)한 뒤, AOSP 테스트 키로 이미지를 재서명하고 부트로더가 수용하도록 `vbmeta.img`를 재구성합니다.

**루팅**은 `boot.img` 또는 `init_boot.img`를 언팩하고, ramdisk에 루팅 관련 파일을 주입한 뒤 리팩하고 원래 AVB 키로 재서명합니다. 부트로더가 테스트 키 서명을 신뢰하기 때문에 수정된 이미지로 부팅됩니다.

**안티롤백 우회**는 Fastboot를 통해 기기의 현재 롤백 인덱스를 읽은 뒤, 대상 펌웨어 이미지를 일치하는 인덱스로 재서명하여 부트로더가 "이전" 빌드를 거부하지 않게 합니다.

**모든 플래싱**은 EDL 모드를 통해 수행됩니다 — LTBox가 전체 흐름을 처리합니다: ADB → Fastboot → EDL 전환, 프로그래머 업로드, 파티션 읽기/쓰기, 재부팅. AVB 서명에는 `avbtool-rs`에 내장된 AOSP `testkey_rsa2048` / `testkey_rsa4096` 스펙을 사용하므로 별도 PEM 파일이 필요 없습니다 — 재서명된 `vbmeta`와 루팅 주입된 `boot` 이미지가 부트로더에 고정된 테스트 키로 검증됩니다.

---

## 🏗️ 프로젝트 구조

| 크레이트 | 역할 |
|---|---|
| `ltbox-core` | 프리미티브 — 에러, 설정, 로깅, GitHub / nightly.link / Lenovo PTSTPD 클라이언트, 암호화, XML 복호화, 라이브 로그 싱크 |
| `ltbox-device` | 전송 계층 — ADB, Fastboot, EDL / QDL, serialport 탐지, Windows 퀄컴 USB 드라이버 감지 + 자동 설치 |
| `ltbox-patch` | 이미지 파이프라인 — AVB(내장 AOSP testkey 스펙), 부트 이미지 ramdisk 패치, 지역 변환, 롤백 인덱스 처리, 루트 프로바이더 통합 |
| `ltbox-gui` | `iced` 데스크톱 앱 — `ltbox.exe` 바이너리 |

---

## 🙏 크레딧

- **익명의 [ㅇㅇ](https://gall.dcinside.com/board/lists?id=tabletpc)**
- **[갓파더](https://ppomppu.co.kr/zboard/view.php?id=androidtab&page=1&divpage=38&no=197457)**
- **[limzei89](https://note.com/limzei89/n/nd5217eb57827)**
- **[hitin911](https://xdaforums.com/m/hitin911.12861404/)**

---

## 📄 라이선스

이 저작물은 [CC BY-NC-SA 4.0][cc-by-nc-sa] 라이선스를 따릅니다.

[![CC BY-NC-SA 4.0][cc-by-nc-sa-image]][cc-by-nc-sa]

[cc-by-nc-sa]: http://creativecommons.org/licenses/by-nc-sa/4.0/
[cc-by-nc-sa-image]: https://licensebuttons.net/l/by-nc-sa/4.0/88x31.png
[cc-by-nc-sa-shield]: https://img.shields.io/badge/License-CC%20BY--NC--SA%204.0-lightgrey.svg
