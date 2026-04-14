from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from typing import Optional


class RootRouteKind(str, Enum):
    DIRECT = "direct"


class RootCommandVariantId(str, Enum):
    LKM = "lkm"
    GKI = "gki"
    APATCH = "apatch"
    FOLKPATCH = "folkpatch"
    MAGISK = "magisk"


class RootProviderFamily(str, Enum):
    LKM = "lkm"
    APATCH = "apatch"
    MAGISK = "magisk"


@dataclass(frozen=True)
class RootCommandVariant:
    variant_id: RootCommandVariantId
    root_device_command: str
    patch_command: Optional[str]
    patch_flash_command: str
    root_menu_root_label_key: str
    root_menu_patch_label_key: str
    task_mode_label: str
    gki: bool
    root_type: str = ""
    title_suffix: str = ""

    def default_kwargs(self) -> dict[str, object]:
        kwargs: dict[str, object] = {"gki": self.gki}
        if self.root_type:
            kwargs["root_type"] = self.root_type
        return kwargs


@dataclass(frozen=True)
class RootProviderProfile:
    provider_id: str
    display_name: str
    family: RootProviderFamily
    settings_key: str
    workflow_file: str
    menu_key: str
    route_kind: RootRouteKind
    menu_label_key: Optional[str] = None
    menu_label_literal: str = ""
    command_variant: Optional[RootCommandVariantId] = None
    strategy_root_type: str = ""
    direct_gki: Optional[bool] = None
    aliases: tuple[str, ...] = ()
    force_nightly: bool = False
    release_uses_tagged_build: bool = False
    nightly_branch: str = "main"
    local_apk_only: bool = False

    @property
    def has_translated_menu_label(self) -> bool:
        return bool(self.menu_label_key)


ROOT_COMMAND_VARIANTS: tuple[RootCommandVariant, ...] = (
    RootCommandVariant(
        variant_id=RootCommandVariantId.LKM,
        root_device_command="root_device_lkm",
        patch_command="patch_root_image_file_lkm",
        patch_flash_command="patch_root_image_file_flash_lkm",
        root_menu_root_label_key="menu_root_1_lkm",
        root_menu_patch_label_key="menu_root_2_lkm",
        task_mode_label="LKM",
        gki=False,
    ),
    RootCommandVariant(
        variant_id=RootCommandVariantId.GKI,
        root_device_command="root_device_gki",
        patch_command="patch_root_image_file_gki",
        patch_flash_command="patch_root_image_file_flash_gki",
        root_menu_root_label_key="menu_root_1_gki",
        root_menu_patch_label_key="menu_root_2_gki",
        task_mode_label="GKI",
        gki=True,
    ),
    RootCommandVariant(
        variant_id=RootCommandVariantId.APATCH,
        root_device_command="root_device_apatch",
        patch_command="patch_root_image_file_apatch",
        patch_flash_command="patch_root_image_file_flash_apatch",
        root_menu_root_label_key="menu_root_1_gki",
        root_menu_patch_label_key="menu_root_2_gki",
        task_mode_label="GKI",
        gki=True,
        root_type="apatch",
        title_suffix="APatch",
    ),
    RootCommandVariant(
        variant_id=RootCommandVariantId.FOLKPATCH,
        root_device_command="root_device_folkpatch",
        patch_command=None,
        patch_flash_command="patch_root_image_file_flash_folkpatch",
        root_menu_root_label_key="menu_root_1_gki",
        root_menu_patch_label_key="menu_root_2_gki",
        task_mode_label="GKI",
        gki=True,
        root_type="folkpatch",
        title_suffix="FolkPatch",
    ),
    RootCommandVariant(
        variant_id=RootCommandVariantId.MAGISK,
        root_device_command="root_device_magisk",
        patch_command="patch_root_image_file_magisk",
        patch_flash_command="patch_root_image_file_flash_magisk",
        root_menu_root_label_key="menu_root_1_lkm",
        root_menu_patch_label_key="menu_root_2_lkm",
        task_mode_label="Magisk",
        gki=False,
        root_type="magisk",
        title_suffix="Magisk",
    ),
)

ROOT_PROFILES: tuple[RootProviderProfile, ...] = (
    RootProviderProfile(
        provider_id="kernelsu",
        display_name="KernelSU",
        family=RootProviderFamily.LKM,
        settings_key="kernelsu",
        workflow_file="build-manager.yml",
        menu_key="1",
        route_kind=RootRouteKind.DIRECT,
        menu_label_key="menu_root_type_ksu",
        command_variant=RootCommandVariantId.LKM,
        strategy_root_type="kernelsu",
        direct_gki=False,
        nightly_branch="main",
    ),
    RootProviderProfile(
        provider_id="kernelsu-next",
        display_name="KernelSU Next",
        family=RootProviderFamily.LKM,
        settings_key="kernelsu-next",
        workflow_file="build-manager-ci.yml",
        menu_key="2",
        route_kind=RootRouteKind.DIRECT,
        menu_label_key="menu_root_type_ksun",
        command_variant=RootCommandVariantId.LKM,
        strategy_root_type="ksu",
        direct_gki=False,
        aliases=("ksu",),
        release_uses_tagged_build=True,
        nightly_branch="dev",
    ),
    RootProviderProfile(
        provider_id="sukisu",
        display_name="SukiSU Ultra",
        family=RootProviderFamily.LKM,
        settings_key="sukisu-ultra",
        workflow_file="build-manager.yml",
        menu_key="3",
        route_kind=RootRouteKind.DIRECT,
        menu_label_key="menu_root_type_sukisu",
        command_variant=RootCommandVariantId.LKM,
        strategy_root_type="sukisu",
        direct_gki=False,
        release_uses_tagged_build=True,
        nightly_branch="main",
    ),
    RootProviderProfile(
        provider_id="resukisu",
        display_name="ReSukiSU",
        family=RootProviderFamily.LKM,
        settings_key="resukisu",
        workflow_file="build-manager.yml",
        menu_key="4",
        route_kind=RootRouteKind.DIRECT,
        menu_label_key="menu_root_type_resukisu",
        command_variant=RootCommandVariantId.LKM,
        strategy_root_type="resukisu",
        direct_gki=False,
        force_nightly=True,
        nightly_branch="main",
    ),
    RootProviderProfile(
        provider_id="apatch",
        display_name="APatch",
        family=RootProviderFamily.APATCH,
        settings_key="apatch",
        workflow_file="build.yml",
        menu_key="5",
        route_kind=RootRouteKind.DIRECT,
        menu_label_literal="APatch",
        command_variant=RootCommandVariantId.APATCH,
        strategy_root_type="apatch",
        direct_gki=True,
        nightly_branch="main",
    ),
    RootProviderProfile(
        provider_id="folkpatch",
        display_name="FolkPatch",
        family=RootProviderFamily.APATCH,
        settings_key="folkpatch",
        workflow_file="build.yml",
        menu_key="6",
        route_kind=RootRouteKind.DIRECT,
        menu_label_literal="FolkPatch",
        command_variant=RootCommandVariantId.FOLKPATCH,
        strategy_root_type="folkpatch",
        direct_gki=True,
        nightly_branch="main",
    ),
    RootProviderProfile(
        provider_id="gki",
        display_name="GKI Mode",
        family=RootProviderFamily.LKM,
        settings_key="gki",
        workflow_file="",
        menu_key="7",
        route_kind=RootRouteKind.DIRECT,
        menu_label_key="menu_root_type_gki",
        command_variant=RootCommandVariantId.GKI,
        strategy_root_type="gki",
        direct_gki=True,
    ),
    RootProviderProfile(
        provider_id="magisk",
        display_name="Magisk",
        family=RootProviderFamily.MAGISK,
        settings_key="magisk",
        workflow_file="ci.yml",
        menu_key="8",
        route_kind=RootRouteKind.DIRECT,
        menu_label_literal="Magisk",
        command_variant=RootCommandVariantId.MAGISK,
        strategy_root_type="magisk",
        direct_gki=False,
        nightly_branch="master",
    ),
    RootProviderProfile(
        provider_id="other_forks",
        display_name="Other forks",
        family=RootProviderFamily.MAGISK,
        settings_key="other_forks",
        workflow_file="",
        menu_key="10",
        route_kind=RootRouteKind.DIRECT,
        menu_label_literal="Other forks",
        command_variant=RootCommandVariantId.MAGISK,
        strategy_root_type="other_forks",
        direct_gki=False,
        local_apk_only=True,
    ),
)

ROOT_TYPE_MENU_LAYOUT: tuple[Optional[str], ...] = (
    "kernelsu",
    "kernelsu-next",
    None,
    "sukisu",
    "resukisu",
    None,
    "apatch",
    "folkpatch",
    None,
    "gki",
    None,
)

_ROOT_COMMAND_VARIANTS_BY_ID = {
    variant.variant_id: variant for variant in ROOT_COMMAND_VARIANTS
}
_ROOT_PROFILES_BY_ID = {profile.provider_id: profile for profile in ROOT_PROFILES}
_ROOT_PROFILES_BY_ALIAS = {
    alias: profile for profile in ROOT_PROFILES for alias in profile.aliases
}


def iter_root_command_variants() -> tuple[RootCommandVariant, ...]:
    return ROOT_COMMAND_VARIANTS


def get_root_command_variant(variant_id: RootCommandVariantId) -> RootCommandVariant:
    return _ROOT_COMMAND_VARIANTS_BY_ID[variant_id]


def iter_root_type_menu_profiles() -> tuple[Optional[RootProviderProfile], ...]:
    layout: list[Optional[RootProviderProfile]] = []
    for provider_id in ROOT_TYPE_MENU_LAYOUT:
        if provider_id is None:
            layout.append(None)
        else:
            layout.append(get_root_provider_profile(provider_id))
    return tuple(layout)


def iter_root_provider_profiles() -> tuple[RootProviderProfile, ...]:
    return ROOT_PROFILES


def get_root_provider_profile(root_type: str) -> RootProviderProfile:
    if root_type in _ROOT_PROFILES_BY_ALIAS:
        return _ROOT_PROFILES_BY_ALIAS[root_type]
    if root_type in _ROOT_PROFILES_BY_ID:
        return _ROOT_PROFILES_BY_ID[root_type]
    raise KeyError(f"Unknown root provider: {root_type}")


def resolve_root_command_variant(gki: bool, root_type: str = "") -> RootCommandVariant:
    if root_type:
        profile = get_root_provider_profile(root_type)
        if profile.command_variant is not None:
            return get_root_command_variant(profile.command_variant)
    return get_root_command_variant(
        RootCommandVariantId.GKI if gki else RootCommandVariantId.LKM
    )
