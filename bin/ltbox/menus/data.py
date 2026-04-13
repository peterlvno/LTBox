from dataclasses import dataclass
from typing import Callable, List, Optional, Union

from ..i18n import get_string
from ..root_profiles import resolve_root_command_variant


@dataclass(frozen=True)
class MenuItem:
    item_type: str
    key: Optional[str] = None
    text: str = ""
    action: Optional[str] = None

    @classmethod
    def option(cls, key: str, text: str, action: Optional[str] = None) -> "MenuItem":
        return cls(item_type="option", key=key, text=text, action=action)

    @classmethod
    def label(cls, text: str) -> "MenuItem":
        return cls(item_type="label", text=text)

    @classmethod
    def separator(cls) -> "MenuItem":
        return cls(item_type="separator")


@dataclass(frozen=True)
class MenuSpec:
    item_type: str
    key: Optional[str] = None
    text: Optional[Union[str, Callable[[], str]]] = None
    action: Optional[str] = None


def _resolve_text(value: Optional[Union[str, Callable[[], str]]]) -> str:
    if callable(value):
        return value()
    return value or ""


def _build_menu(specs: List[MenuSpec]) -> List[MenuItem]:
    items: List[MenuItem] = []
    for spec in specs:
        if spec.item_type == "separator":
            items.append(MenuItem.separator())
        elif spec.item_type == "label":
            items.append(MenuItem.label(_resolve_text(spec.text)))
        elif spec.item_type == "option":
            items.append(
                MenuItem.option(
                    spec.key or "",
                    _resolve_text(spec.text),
                    action=spec.action,
                )
            )
    return items


def _navigation_specs(
    *,
    include_back: bool = False,
    include_return: bool = False,
    include_exit: bool = False,
    return_label_key: str = "menu_root_m",
) -> List[MenuSpec]:
    specs: List[MenuSpec] = []
    if include_back:
        specs.append(
            MenuSpec(
                "option", key="b", text=lambda: get_string("menu_back"), action="back"
            )
        )
    if include_return:
        specs.append(
            MenuSpec(
                "option",
                key="m",
                text=lambda: get_string(return_label_key),
                action="return",
            )
        )
    if include_exit:
        specs.append(
            MenuSpec(
                "option",
                key="x",
                text=lambda: get_string("menu_main_exit"),
                action="exit",
            )
        )
    return specs


def get_advanced_menu_data(
    target_region: str, modify_region_code_enabled: bool = True
) -> List[MenuItem]:
    region_text = (
        get_string("menu_adv_1_row")
        if target_region == "ROW"
        else get_string("menu_adv_1_prc")
    )

    specs: List[MenuSpec] = []
    if modify_region_code_enabled:
        specs.extend(
            [
                MenuSpec("option", key="1", text=region_text, action="convert"),
                MenuSpec("separator"),
            ]
        )

    specs.extend(
        [
            MenuSpec(
                "option",
                key="2",
                text=lambda: get_string("menu_adv_2"),
                action="dump_partitions",
            ),
            MenuSpec(
                "option",
                key="3",
                text=lambda: get_string("menu_adv_3"),
                action="edit_dp",
            ),
            MenuSpec(
                "option",
                key="4",
                text=lambda: get_string("menu_adv_4"),
                action="flash_partitions",
            ),
            MenuSpec("separator"),
            MenuSpec(
                "option",
                key="5",
                text=lambda: get_string("menu_adv_5"),
                action="read_anti_rollback",
            ),
            MenuSpec(
                "option",
                key="6",
                text=lambda: get_string("menu_adv_6"),
                action="patch_anti_rollback",
            ),
            MenuSpec(
                "option",
                key="7",
                text=lambda: get_string("menu_adv_7"),
                action="write_anti_rollback",
            ),
            MenuSpec("separator"),
            MenuSpec(
                "option",
                key="8",
                text=lambda: get_string("menu_adv_8"),
                action="decrypt_xml",
            ),
            MenuSpec(
                "option",
                key="9",
                text=lambda: get_string("task_title_modify_xml_wipe"),
                action="modify_xml_wipe",
            ),
            MenuSpec(
                "option",
                key="10",
                text=lambda: get_string("task_title_modify_xml_nowipe"),
                action="modify_xml",
            ),
            MenuSpec("separator"),
            MenuSpec(
                "option",
                key="11",
                text=lambda: get_string("menu_adv_11"),
                action="flash_full_firmware",
            ),
            MenuSpec(
                "option",
                key="12",
                text=lambda: get_string("menu_adv_12"),
                action="flash_selected_partitions",
            ),
            MenuSpec(
                "option",
                key="13",
                text=lambda: get_string("task_title_rebuild_vbmeta"),
                action="rebuild_vbmeta",
            ),
            MenuSpec(
                "option",
                key="14",
                text=lambda: get_string("menu_main_rec_flash"),
                action="sign_and_flash_recovery",
            ),
            MenuSpec("separator"),
            *_navigation_specs(include_back=True, include_exit=True),
        ]
    )
    return _build_menu(specs)


def get_root_variants_menu_data() -> List[MenuItem]:
    specs = [
        MenuSpec(
            "option",
            key="1",
            text=lambda: get_string("menu_root_variants_magisk"),
            action="magisk_variants",
        ),
        MenuSpec(
            "option",
            key="2",
            text=lambda: get_string("menu_root_variants_ksu"),
            action="ksu_variants",
        ),
        MenuSpec(
            "option",
            key="3",
            text=lambda: get_string("menu_root_variants_apatch"),
            action="apatch_variants",
        ),
        MenuSpec("separator"),
        *_navigation_specs(include_back=True),
    ]
    return _build_menu(specs)


def get_root_magisk_variants_menu_data() -> List[MenuItem]:
    specs = [
        MenuSpec(
            "option",
            key="1",
            text=lambda: "Magisk",
            action="magisk",
        ),
        MenuSpec(
            "option",
            key="2",
            text=lambda: get_string("menu_root_magisk_other"),
            action="other_forks",
        ),
        MenuSpec("separator"),
        *_navigation_specs(include_back=True, include_return=True),
    ]
    return _build_menu(specs)


def get_root_ksu_modes_menu_data() -> List[MenuItem]:
    specs = [
        MenuSpec(
            "option",
            key="1",
            text=lambda: get_string("menu_root_mode_lkm"),
            action="lkm_mode",
        ),
        MenuSpec(
            "option",
            key="2",
            text=lambda: get_string("menu_root_mode_gki"),
            action="gki_mode",
        ),
        MenuSpec("separator"),
        *_navigation_specs(include_back=True, include_return=True),
    ]
    return _build_menu(specs)


def get_root_apatch_variants_menu_data() -> List[MenuItem]:
    specs = [
        MenuSpec(
            "option",
            key="1",
            text=lambda: "APatch",
            action="apatch",
        ),
        MenuSpec(
            "option",
            key="2",
            text=lambda: "FolkPatch",
            action="folkpatch",
        ),
        MenuSpec("separator"),
        *_navigation_specs(include_back=True, include_return=True),
    ]
    return _build_menu(specs)


def get_root_menu_data(gki: bool, root_type: str = "") -> List[MenuItem]:
    variant = resolve_root_command_variant(gki, root_type)
    specs: List[MenuSpec] = [
        MenuSpec(
            "option",
            key="1",
            text=lambda: get_string(variant.root_menu_root_label_key),
            action=variant.root_device_command,
        ),
        MenuSpec(
            "option",
            key="2",
            text=lambda: get_string(variant.root_menu_patch_label_key),
            action=variant.patch_flash_command,
        ),
    ]

    specs.extend(
        [
            MenuSpec("separator"),
            *_navigation_specs(include_back=True, include_return=True),
        ]
    )
    return _build_menu(specs)


def get_settings_menu_data(
    preset_label: str,
    skip_adb_state: str,
    modify_region_code_enabled: bool,
    target_region: str,
    modify_rollback_index: str = "ON",
) -> List[MenuItem]:
    region_label = (
        get_string("menu_settings_device_row")
        if target_region == "ROW"
        else get_string("menu_settings_device_prc")
    )

    specs = [
        MenuSpec(
            "option",
            key="1",
            text=lambda: get_string("menu_settings_preset").format(preset=preset_label),
            action="select_preset",
        ),
        MenuSpec(
            "option",
            key="2",
            text=lambda: get_string("menu_settings_modify_region").format(
                state="ON" if modify_region_code_enabled else "OFF"
            ),
            action="toggle_modify_region_code",
        ),
    ]

    if modify_region_code_enabled:
        specs.append(
            MenuSpec("option", key="3", text=region_label, action="toggle_region")
        )

    specs.extend(
        [
            MenuSpec(
                "option",
                key="4",
                text=lambda: get_string("menu_settings_modify_rb").format(
                    state=modify_rollback_index
                ),
                action="cycle_rollback",
            ),
            MenuSpec("separator"),
            MenuSpec(
                "option",
                key="5",
                text=lambda: get_string("menu_settings_skip_adb").format(
                    state=skip_adb_state
                ),
                action="toggle_adb",
            ),
            MenuSpec("separator"),
            MenuSpec(
                "option",
                key="6",
                text=lambda: (
                    f"{get_string('menu_settings_lang')}: [{get_string('_lang')}]"
                ),
                action="change_lang",
            ),
            MenuSpec(
                "option",
                key="7",
                text=lambda: get_string("menu_settings_check_update"),
                action="check_update",
            ),
            MenuSpec("separator"),
            *_navigation_specs(include_back=True),
        ]
    )
    return _build_menu(specs)


def get_main_menu_data(
    target_region: str, modify_region_code_enabled: bool = True
) -> List[MenuItem]:
    if not modify_region_code_enabled:
        install_wipe_text = get_string("menu_main_install_wipe")
        install_keep_text = get_string("menu_main_install_keep")
    elif target_region == "ROW":
        install_wipe_text = get_string("menu_main_install_wipe_row")
        install_keep_text = get_string("menu_main_install_keep_row")
    else:
        install_wipe_text = get_string("menu_main_install_wipe_prc")
        install_keep_text = get_string("menu_main_install_keep_prc")

    specs = [
        MenuSpec("option", key="1", text=install_wipe_text, action="patch_all_wipe"),
        MenuSpec("option", key="2", text=install_keep_text, action="patch_all"),
        MenuSpec("separator"),
        MenuSpec(
            "option",
            key="3",
            text=lambda: get_string("menu_main_disable_ota"),
            action="disable_ota",
        ),
        MenuSpec(
            "option",
            key="4",
            text=lambda: get_string("menu_main_reenable_ota"),
            action="reenable_ota",
        ),
        MenuSpec(
            "option",
            key="5",
            text=lambda: get_string("menu_main_rescue"),
            action="rescue_ota",
        ),
        MenuSpec("separator"),
        MenuSpec(
            "option",
            key="6",
            text=lambda: get_string("menu_main_root"),
            action="menu_root",
        ),
        MenuSpec(
            "option",
            key="7",
            text=lambda: get_string("menu_main_unroot"),
            action="unroot_device",
        ),
        MenuSpec("separator"),
        MenuSpec(
            "option",
            key="r",
            text=lambda: get_string("menu_main_reboot"),
            action="menu_reboot",
        ),
        MenuSpec("separator"),
        MenuSpec(
            "option",
            key="0",
            text=lambda: get_string("menu_settings_title"),
            action="menu_settings",
        ),
        MenuSpec(
            "option",
            key="a",
            text=lambda: get_string("menu_main_adv"),
            action="menu_advanced",
        ),
        *_navigation_specs(include_exit=True),
    ]
    return _build_menu(specs)


def get_reboot_menu_data(device_status_key: str) -> List[MenuItem]:
    specs: List[MenuSpec] = []

    if device_status_key == "device_status_adb":
        specs.extend(
            [
                MenuSpec(
                    "option",
                    key="1",
                    text=lambda: get_string("menu_reboot_system"),
                    action="reboot_adb_system",
                ),
                MenuSpec(
                    "option",
                    key="2",
                    text=lambda: get_string("menu_reboot_fastboot"),
                    action="reboot_adb_bootloader",
                ),
                MenuSpec(
                    "option",
                    key="3",
                    text=lambda: get_string("menu_reboot_fastbootd"),
                    action="reboot_adb_fastboot",
                ),
                MenuSpec(
                    "option",
                    key="4",
                    text=lambda: get_string("menu_reboot_edl"),
                    action="reboot_adb_edl",
                ),
            ]
        )
    elif device_status_key == "device_status_fastboot":
        specs.extend(
            [
                MenuSpec(
                    "option",
                    key="1",
                    text=lambda: get_string("menu_reboot_system"),
                    action="reboot_fb_system",
                ),
                MenuSpec(
                    "option",
                    key="2",
                    text=lambda: get_string("menu_reboot_fastboot"),
                    action="reboot_fb_bootloader",
                ),
                MenuSpec(
                    "option",
                    key="3",
                    text=lambda: get_string("menu_reboot_fastbootd"),
                    action="reboot_fb_fastboot",
                ),
                MenuSpec(
                    "option",
                    key="4",
                    text=lambda: get_string("menu_reboot_edl"),
                    action="reboot_fb_edl",
                ),
            ]
        )
    elif device_status_key == "device_status_edl":
        specs.append(
            MenuSpec(
                "option",
                key="1",
                text=lambda: get_string("menu_reboot_system"),
                action="reboot_edl_system",
            )
        )
    elif device_status_key == "device_status_adb_required":
        specs.append(
            MenuSpec("label", text=lambda: get_string("menu_reboot_adb_required"))
        )
    else:
        specs.append(
            MenuSpec("label", text=lambda: get_string("menu_reboot_not_recognized"))
        )

    specs.extend(
        [
            MenuSpec("separator"),
            *_navigation_specs(include_back=True),
        ]
    )
    return _build_menu(specs)
