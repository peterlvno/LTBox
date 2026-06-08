# LTBox

[🇺🇸 English](../README.md) / [🇨🇳 简体中文](README_zh-CN.md)

[![License: CC BY-NC-SA 4.0][cc-by-nc-sa-shield]][cc-by-nc-sa]

## ⚠️ 면책 조항

**교육 목적으로만 사용하세요.** 펌웨어 수정은 벽돌, 데이터 손실, 보증 무효화 등의 위험이 있습니다. 제작자는 **어떠한 책임도 지지 않습니다**. 모든 결과는 사용자 본인의 책임입니다. **본인의 위험 부담 하에 사용하세요.**

---

## 🚀 빠른 시작

![Windows](https://img.shields.io/badge/Windows-0078D6?logo=windows&logoColor=white) ![Linux](https://img.shields.io/badge/Linux-FCC624?logo=linux&logoColor=black) ![macOS](https://img.shields.io/badge/macOS-000000?logo=apple&logoColor=white)

위키의 **[빠른 시작](https://github.com/miner7222/LTBox/wiki/Home#quick-start)**을 참고하세요.

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

<details>
<summary>파이프라인 개별 단계 수동 제어, 세 섹션으로 구성</summary>

<br>

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

</details>

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
