from dataclasses import dataclass, field
from typing import Any, Callable, Dict, List, Optional

from . import actions, workflow
from .i18n import get_string
from .registry import REGISTRY
from .root_profiles import RootCommandVariant, iter_root_command_variants


@dataclass(frozen=True)
class CommandDefinition:
    name: str
    func: Callable[..., Any]
    title: str
    require_dev: bool = True
    default_kwargs: Dict[str, Any] = field(default_factory=dict)
    result_handler: Optional[Callable[[Any], Any]] = None
    log_filename_prefix: Optional[str] = None


def _build_command_definitions() -> List[CommandDefinition]:
    return [
        CommandDefinition(
            name="convert",
            func=actions.convert_region_images,
            title=get_string("task_title_convert_rom"),
        ),
        *_build_root_command_definitions(),
        CommandDefinition(
            name="unroot_device",
            func=actions.unroot_device,
            title=get_string("task_title_unroot"),
        ),
        CommandDefinition(
            name="rebuild_vbmeta",
            func=actions.rebuild_vbmeta,
            title=get_string("task_title_rebuild_vbmeta"),
        ),
        CommandDefinition(
            name="sign_and_flash_recovery",
            func=actions.sign_and_flash_recovery,
            title=get_string("task_title_rec_flash"),
        ),
        CommandDefinition(
            name="disable_ota",
            func=actions.disable_ota,
            title=get_string("task_title_disable_ota"),
        ),
        CommandDefinition(
            name="reenable_ota",
            func=actions.reenable_ota,
            title=get_string("task_title_reenable_ota"),
        ),
        CommandDefinition(
            name="rescue_ota",
            func=actions.rescue_after_ota,
            title=get_string("task_title_rescue"),
        ),
        CommandDefinition(
            name="edit_dp",
            func=actions.edit_devinfo_persist,
            title=get_string("task_title_patch_devinfo"),
            require_dev=False,
        ),
        CommandDefinition(
            name="dump_partitions",
            func=actions.dump_partitions,
            title=get_string("task_title_dump_devinfo"),
        ),
        CommandDefinition(
            name="flash_partitions",
            func=actions.flash_partitions,
            title=get_string("task_title_write_devinfo"),
        ),
        CommandDefinition(
            name="read_anti_rollback",
            func=actions.read_device_anti_rollback,
            title=get_string("task_title_read_arb"),
        ),
        CommandDefinition(
            name="patch_anti_rollback",
            func=actions.patch_rom_anti_rollback,
            title=get_string("task_title_patch_arb"),
            require_dev=False,
        ),
        CommandDefinition(
            name="write_anti_rollback",
            func=actions.write_anti_rollback,
            title=get_string("task_title_write_arb"),
        ),
        CommandDefinition(
            name="decrypt_xml",
            func=actions.decrypt_x_files,
            title=get_string("task_title_decrypt_xml"),
            require_dev=False,
        ),
        CommandDefinition(
            name="modify_xml",
            func=actions.modify_xml,
            title=get_string("task_title_modify_xml_nowipe"),
            require_dev=False,
            default_kwargs={"wipe": 0},
        ),
        CommandDefinition(
            name="modify_xml_wipe",
            func=actions.modify_xml,
            title=get_string("task_title_modify_xml_wipe"),
            require_dev=False,
            default_kwargs={"wipe": 1},
        ),
        CommandDefinition(
            name="flash_full_firmware",
            func=actions.flash_full_firmware,
            title=get_string("task_title_flash_full_firmware"),
        ),
        CommandDefinition(
            name="flash_selected_partitions",
            func=actions.flash_selected_partitions,
            title=get_string("task_title_flash_partitions_label"),
        ),
        CommandDefinition(
            name="patch_all",
            func=workflow.patch_all,
            title=get_string("task_title_install_nowipe"),
            default_kwargs={"wipe": 0, "manage_execution": False},
            log_filename_prefix="log_flash_firmware",
        ),
        CommandDefinition(
            name="patch_all_wipe",
            func=workflow.patch_all,
            title=get_string("task_title_install_wipe"),
            default_kwargs={"wipe": 1, "manage_execution": False},
            log_filename_prefix="log_flash_firmware",
        ),
    ]


def _build_root_command_definitions() -> List[CommandDefinition]:
    command_definitions: List[CommandDefinition] = []
    for variant in iter_root_command_variants():
        command_definitions.extend(_build_root_variant_command_definitions(variant))
    return command_definitions


def _build_root_variant_command_definitions(
    variant: RootCommandVariant,
) -> List[CommandDefinition]:
    title_suffix = f" ({variant.title_suffix})" if variant.title_suffix else ""
    patch_title_key = (
        "task_title_root_file_gki" if variant.gki else "task_title_root_file_lkm"
    )
    default_kwargs = variant.default_kwargs()

    command_definitions = [
        CommandDefinition(
            name=variant.root_device_command,
            func=actions.root_device,
            title=get_string("task_title_root_device").format(
                mode=variant.task_mode_label
            )
            + title_suffix,
            default_kwargs=default_kwargs.copy(),
        ),
        CommandDefinition(
            name=variant.patch_flash_command,
            func=actions.patch_and_flash_root,
            title=get_string(patch_title_key) + title_suffix,
            default_kwargs=default_kwargs.copy(),
        ),
    ]

    if variant.patch_command:
        command_definitions.insert(
            1,
            CommandDefinition(
                name=variant.patch_command,
                func=actions.patch_root_image_file,
                title=get_string(patch_title_key) + title_suffix,
                require_dev=False,
                default_kwargs=default_kwargs.copy(),
            ),
        )

    return command_definitions


def register_all_commands() -> None:
    command_definitions = _build_command_definitions()

    seen_names = set()
    for command in command_definitions:
        if command.name in seen_names:
            raise ValueError(f"Duplicate command registration: {command.name}")
        seen_names.add(command.name)

        REGISTRY.add(
            command.name,
            command.func,
            command.title,
            require_dev=command.require_dev,
            result_handler=command.result_handler,
            log_filename_prefix=command.log_filename_prefix,
            **command.default_kwargs,
        )
