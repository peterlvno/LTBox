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
- ⚡ **파티션 플래싱** — EDL(Emergency Download) 모드를 통한 개별 파티션 읽기/쓰기

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

1. [최신 릴리즈](../../releases/latest)를 다운로드하고 압축 해제 (경로에 공백/특수문자 없이)
2. **`start.bat`** 더블클릭
3. 화면의 메뉴를 따라 진행

---

## 📋 무엇을 할 수 있나요?

### 메인 메뉴

| 옵션 | 설명 |
|---|---|
| **펌웨어 설치 (초기화/유지)** | 올인원: 지역 변환 → 패치 → 플래싱. 데이터 초기화 또는 유지 |
| **시스템 업데이트 비활성화/활성화** | ADB를 통해 OTA 업데이트 차단 또는 복원 |
| **부팅 실패 복구** | 변환된 기기에서 OTA 후 부팅 문제 수정 |
| **루팅** | KernelSU / KernelSU Next / SukiSU / ReSukiSU / APatch / FolkPatch로 루팅 |
| **루팅 해제** | 백업에서 순정 부트 이미지 복원 |
| **설정** | 프리셋, 지역, 롤백, 언어, ADB 건너뛰기 |
| **고급 메뉴** | 개별 단계 — 아래 참조 |

### 루트 프로바이더

**Magisk 계열** — 기존 ramdisk 인젝션

| 프로바이더 |
|---|
| Magisk |
| 기타 포크 |

**KernelSU 계열** — LKM (로더블 커널 모듈) 또는 GKI (커스텀 커널) 모드

| 프로바이더 | LKM | GKI |
|---|---|---|
| KernelSU | ✅ | ✅ |
| KernelSU Next | ✅ | ✅ |
| SukiSU Ultra | ✅ | ✅ |
| ReSukiSU | ✅ | ✅ |

**APatch 계열** — 부트 이미지 직접 패치 (GKI)

| 프로바이더 |
|---|
| APatch |
| FolkPatch |

> Y700 2세대는 KernelSU 계열의 GKI 모드와 APatch 계열만 지원합니다.

### 고급 메뉴

개별 단계 수동 제어:

- 지역 변환 (vendor_boot + vbmeta 재구성)
- devinfo & persist 덤프 / 패치 / 플래싱
- 안티롤백 인덱스 감지 및 패치
- `.x` 파일 복호화 → XML
- 플래싱용 XML 수정 (초기화 또는 데이터 유지)
- EDL을 통한 펌웨어 또는 선택 파티션 플래싱
- 수정된 이미지에 대한 vbmeta 재구성
- 커스텀 리커버리 서명 및 플래싱

---

## 🔧 작동 원리 (간략)

**지역 변환**은 `vendor_boot.img`의 바이트를 패치(PRC↔ROW 지역 식별자)한 뒤, AOSP 테스트 키로 이미지를 재서명하고 부트로더가 수용하도록 `vbmeta.img`를 재구성합니다.

**루팅**은 `boot.img` 또는 `init_boot.img`를 언팩하고, ramdisk(CPIO 아카이브)에 루팅 관련 파일을 주입한 뒤 리팩하고 원래 AVB 키로 재서명합니다. 부트로더가 테스트 키 서명을 신뢰하기 때문에 수정된 이미지로 부팅됩니다.

**안티롤백 우회**는 Fastboot를 통해 기기의 현재 롤백 인덱스를 읽은 뒤, 대상 펌웨어 이미지를 일치하는 인덱스로 재서명하여 부트로더가 "이전" 빌드를 거부하지 않게 합니다.

**모든 플래싱**은 EDL(Qualcomm Emergency Download) 모드를 통해 수행됩니다 — LTBox가 전체 흐름을 처리합니다: ADB → Fastboot → EDL 전환, 프로그래머 업로드, 파티션 읽기/쓰기, 재부팅

---

## 🛠️ 유틸리티

**`info_image.bat`** — `.img` 파일이나 폴더를 드래그 앤 드롭하여 AVB 메타데이터를 확인합니다.

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
