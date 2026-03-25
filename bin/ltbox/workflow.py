import shutil
from dataclasses import dataclass
from datetime import datetime
from typing import Callable, Optional

from . import actions
from . import constants as const
from . import device, utils
from .context import TaskContext
from .errors import (
    DeviceCommandError,
    DeviceError,
    LTBoxError,
    UserCancelError,
)
from .i18n import get_string
from .logger import logging_context


def _cleanup_previous_outputs(ctx: TaskContext) -> None:
    output_folders_to_clean = [
        const.OUTPUT_DIR,
        const.OUTPUT_ROOT_DIR,
        const.OUTPUT_DP_DIR,
        const.OUTPUT_ANTI_ROLLBACK_DIR,
        const.OUTPUT_XML_DIR,
    ]

    for folder in output_folders_to_clean:
        if folder.exists():
            try:
                shutil.rmtree(folder)
            except OSError as e:
                raise LTBoxError(
                    get_string("utils_remove_error").format(name=folder.name, e=e), e
                )


def _populate_device_info(ctx: TaskContext) -> None:
    ctx.active_slot_suffix = ctx.dev.detect_active_slot()

    try:
        ctx.device_model = ctx.dev.fastboot.get_model()
        if not ctx.device_model:
            raise DeviceError(get_string("wf_err_fastboot_model"))
    except DeviceCommandError as e:
        raise DeviceError(get_string("wf_err_get_model").format(e=e), e)


def _wait_for_input_images(ctx: TaskContext) -> None:
    prompt = get_string("act_prompt_image")
    utils.wait_for_directory(const.IMAGE_DIR, prompt)


def _convert_region_images(ctx: TaskContext) -> None:
    actions.convert_region_images(
        dev=ctx.dev,
        device_model=ctx.device_model,
        target_region=ctx.target_region,
        modify_region_code=ctx.modify_region_code,
        on_log=ctx.on_log,
    )


def _decrypt_and_modify_xml(ctx: TaskContext) -> None:
    actions.decrypt_x_files()
    actions.modify_xml(wipe=ctx.wipe)


def _detect_anti_rollback(ctx: TaskContext) -> None:
    if ctx.modify_rollback_index == "OFF":
        ctx.skip_rollback = True
        ctx.on_log(get_string("wf_arb_detect_skipped"))
        return

    # ON/AUTO: anti-rollback is checked from dumped images in the ARB step.


def _dump_images(ctx: TaskContext) -> None:
    ctx.skip_dp_workflow = ctx.wipe == 0

    suffix = ctx.active_slot_suffix if ctx.active_slot_suffix else ""
    ctx.boot_target = f"boot{suffix}"
    ctx.vbmeta_target = f"vbmeta_system{suffix}"
    extra_dumps = []
    if not ctx.skip_rollback:
        extra_dumps = [ctx.boot_target, ctx.vbmeta_target]

    if (not ctx.skip_dp_workflow) or extra_dumps:
        actions.dump_partitions(
            dev=ctx.dev,
            skip_reset=False,
            additional_targets=extra_dumps,
            default_targets=not ctx.skip_dp_workflow,
        )


def _patch_devinfo(ctx: TaskContext) -> None:
    if not ctx.skip_dp_workflow:
        ctx.backup_dir_name = actions.edit_devinfo_persist(
            on_log=ctx.on_log,
            on_confirm=lambda msg: (
                utils.ui.prompt(msg + " (y/n) ").lower().strip() == "y"
            ),
        )


def _check_and_patch_arb(ctx: TaskContext) -> None:
    if not ctx.boot_target or not ctx.vbmeta_target:
        raise LTBoxError(get_string("wf_err_halted"))

    dumped_boot = const.BACKUP_DIR / f"{ctx.boot_target}.img"
    dumped_vbmeta = const.BACKUP_DIR / f"{ctx.vbmeta_target}.img"

    arb_status_result = actions.read_anti_rollback(
        dumped_boot_path=dumped_boot, dumped_vbmeta_path=dumped_vbmeta
    )

    if arb_status_result[0] == "ERROR":
        raise LTBoxError(get_string("wf_step8_err_arb_abort"))

    from .actions.arb import ArbStatus

    status, boot_rb, vbmeta_rb = arb_status_result

    if vbmeta_rb == 0:
        ctx.skip_rollback = True
        ctx.on_log(get_string("wf_arb_no_protection"))
        return

    if ctx.modify_rollback_index == "ON" and status == ArbStatus.MATCH:
        status = ArbStatus.NEEDS_PATCH
        arb_status_result = (status, boot_rb, vbmeta_rb)

    if status == ArbStatus.NEEDS_PATCH:
        ctx.arb_patched = True

    actions.patch_anti_rollback(comparison_result=arb_status_result)


def _flash_images(ctx: TaskContext) -> None:
    actions.flash_full_firmware(
        dev=ctx.dev, skip_reset_edl=True, skip_dp=ctx.skip_dp_workflow
    )


def _log_workflow_halt() -> None:
    utils.ui.echo(get_string("wf_err_halted"), err=True)


@dataclass(frozen=True)
class WorkflowStep:
    label_key: Optional[str]
    action: Callable[[], None]
    after_label_key: Optional[str] = None


def _run_step(ctx: TaskContext, step: WorkflowStep) -> None:
    if step.label_key:
        ctx.on_log(get_string(step.label_key))
    step.action()
    if step.after_label_key:
        ctx.on_log(get_string(step.after_label_key))


def _run_steps(ctx: TaskContext, steps: list[WorkflowStep]) -> None:
    for step in steps:
        _run_step(ctx, step)


def _run_dump_step(ctx: TaskContext) -> None:
    _dump_images(ctx)


def _run_patch_dp_step(ctx: TaskContext) -> None:
    if ctx.skip_dp_workflow:
        ctx.on_log(get_string("wf_step7_skipped"))
        return

    ctx.on_log(get_string("wf_step7_patch_dp"))
    _patch_devinfo(ctx)


def _run_arb_step(ctx: TaskContext) -> None:
    if ctx.skip_rollback:
        ctx.on_log(get_string("wf_step8_skipped"))
        return

    ctx.on_log(get_string("wf_step8_check_arb"))
    _check_and_patch_arb(ctx)


def _build_steps(ctx: TaskContext) -> list[WorkflowStep]:
    return [
        WorkflowStep("wf_step1_clean", lambda: _cleanup_previous_outputs(ctx)),
        WorkflowStep("wf_step2_device_info", lambda: _populate_device_info(ctx)),
        WorkflowStep(None, lambda: _log_active_slot(ctx)),
        WorkflowStep(
            "wf_step3_wait_image",
            lambda: _wait_for_input_images(ctx),
            after_label_key="wf_step3_found",
        ),
        WorkflowStep("wf_step4_convert", lambda: _convert_region_images(ctx)),
        WorkflowStep("wf_step5_modify_xml", lambda: _decrypt_and_modify_xml(ctx)),
        WorkflowStep(None, lambda: _detect_anti_rollback(ctx)),
        WorkflowStep("wf_step6_dump", lambda: _run_dump_step(ctx)),
        WorkflowStep(None, lambda: _run_patch_dp_step(ctx)),
        WorkflowStep(None, lambda: _run_arb_step(ctx)),
        WorkflowStep("wf_step9_flash", lambda: _flash_images(ctx)),
    ]


def _log_active_slot(ctx: TaskContext) -> None:
    active_slot_str = (
        ctx.active_slot_suffix
        if ctx.active_slot_suffix
        else get_string("wf_active_slot_unknown")
    )
    ctx.on_log(get_string("act_active_slot").format(slot=active_slot_str))


def patch_all(
    dev: device.DeviceController,
    wipe: int = 0,
    modify_region_code: bool = True,
    target_region: str = "PRC",
    modify_rollback_index: str = "ON",
) -> str:
    ctx = TaskContext(
        dev=dev,
        wipe=wipe,
        modify_rollback_index=modify_rollback_index,
        modify_region_code=modify_region_code,
        target_region=target_region,
        on_log=lambda s: utils.ui.info(s),
    )

    timestamp = datetime.now().strftime("%Y%m%d_%H%M%S")
    log_dir = const.BASE_DIR / "log"
    log_dir.mkdir(parents=True, exist_ok=True)
    log_file = str(log_dir / f"log_flash_firmware_{timestamp}.txt")
    command_name = "patch_all_wipe" if wipe == 1 else "patch_all"

    utils.ui.info(get_string("logging_enabled").format(log_file=log_file))
    utils.ui.info(get_string("logging_command").format(command=command_name))

    if target_region == "ROW":
        utils.ui.info(get_string("menu_main_install_keep_row"))
    else:
        utils.ui.info(get_string("menu_main_install_keep_prc"))

    try:
        with logging_context(log_file):
            if ctx.wipe == 1:
                ctx.on_log(get_string("wf_wipe_mode_start"))
            else:
                ctx.on_log(get_string("wf_nowipe_mode_start"))
            _run_steps(ctx, _build_steps(ctx))

            success_msg = get_string("wf_process_complete")
            success_msg += f"\n{get_string('wf_process_complete_info')}"

            if ctx.backup_dir_name:
                success_msg += f"\n\n{get_string('wf_backup_notice').format(dir=ctx.backup_dir_name)}"

            if ctx.arb_patched:
                success_msg += f"\n\n{get_string('wf_arb_patched_warning')}"

            success_msg += f"\n\n{get_string('wf_notice_widevine')}"
            return success_msg

    except KeyboardInterrupt as e:
        _log_workflow_halt()
        raise UserCancelError(get_string("act_op_cancel")) from e
    except SystemExit as e:
        _log_workflow_halt()
        raise LTBoxError(get_string("wf_err_halted_script").format(e=e), e) from e
    except Exception:
        _log_workflow_halt()
        raise
    finally:
        utils.ui.info(get_string("logging_finished").format(log_file=log_file))
