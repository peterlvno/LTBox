import shutil
from dataclasses import dataclass
from typing import Callable, Literal, Optional

from . import actions
from .actions.arb import (
    ArbResult,
    ArbStatus,
    check_image_folder_arb,
    compute_device_rollback_index,
)
from . import constants as const
from . import device, utils
from .context import TaskContext
from .execution import (
    TaskResult,
    announce_logging_finished,
    announce_logging_start,
    build_log_filename,
)
from .errors import (
    DeviceCommandError,
    DeviceError,
    LTBoxError,
    UserCancelError,
)
from .i18n import get_string
from .logger import logging_context
from .menus.workflow_prompts import UiWorkflowPrompts, WorkflowPrompts
from .part.backups import find_backup_critical_dirs


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
    ctx.dev.ensure_fastboot_mode()

    try:
        fb_vars = ctx.dev.fastboot.get_all_vars()
    except DeviceCommandError as e:
        raise DeviceError(get_string("wf_err_get_model").format(e=e), e)

    ctx.active_slot_suffix = fb_vars.slot_suffix
    ctx.device_model = fb_vars.model
    ctx.serialno = fb_vars.serialno

    if not ctx.device_model:
        raise DeviceError(get_string("wf_err_fastboot_model"))

    if fb_vars.stored_rollback_indices:
        ctx.device_rollback_index = compute_device_rollback_index(
            fb_vars.stored_rollback_indices
        )


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

    if ctx.device_rollback_index is not None:
        ctx.on_log(
            get_string("wf_arb_detected_device_index").format(
                index=ctx.device_rollback_index
            )
        )
        return

    if ctx.device_model == "TB320FC":
        ctx.tb320fc_arb_fallback = True
        ctx.on_log(get_string("wf_arb_tb320fc_fallback"))
        return

    ctx.skip_rollback = True
    ctx.on_log(get_string("wf_arb_no_stored_indices"))


def _check_backup_critical(ctx: TaskContext) -> None:
    if ctx.modify_region_code:
        return

    backup_dirs = find_backup_critical_dirs(const.BASE_DIR)
    if not backup_dirs:
        return

    backup_choice = (ctx.prompts or UiWorkflowPrompts()).choose_backup_source(
        backup_dirs
    )
    if backup_choice.force_dump:
        ctx.force_dp_workflow = True
        ctx.skip_dp_flash = False
        return
    if backup_choice.skip_all:
        ctx.skip_dp_workflow = True
        ctx.skip_dp_flash = True
        ctx.use_backup_dp = False
        return
    if backup_choice.selected_dir is None:
        return

    chosen = backup_choice.selected_dir
    ctx.on_log(get_string("act_found_patched_folder").format(dir=chosen.name))
    if const.OUTPUT_DP_DIR.exists():
        shutil.rmtree(const.OUTPUT_DP_DIR)
    const.OUTPUT_DP_DIR.mkdir(exist_ok=True)
    for img in chosen.glob("*.img"):
        shutil.copy(img, const.OUTPUT_DP_DIR / img.name)
    ctx.use_backup_dp = True
    ctx.backup_dir_name = chosen.name


def _should_skip_dp_workflow(ctx: TaskContext) -> bool:
    return (
        ctx.use_backup_dp
        or ctx.skip_dp_flash
        or (ctx.wipe == 0 and not ctx.force_dp_workflow)
    )


def _dump_images(ctx: TaskContext) -> None:
    ctx.skip_dp_workflow = _should_skip_dp_workflow(ctx)

    suffix = ctx.active_slot_suffix if ctx.active_slot_suffix else ""
    ctx.boot_target = f"boot{suffix}"
    ctx.vbmeta_target = f"vbmeta_system{suffix}"
    extra_dumps = []
    if ctx.tb320fc_arb_fallback:
        extra_dumps = [ctx.boot_target, ctx.vbmeta_target]

    if (not ctx.skip_dp_workflow) or extra_dumps:
        actions.dump_partitions(
            dev=ctx.dev,
            skip_reset=True,
            additional_targets=extra_dumps,
            default_targets=not ctx.skip_dp_workflow,
        )


def _patch_devinfo(ctx: TaskContext) -> None:
    if not ctx.skip_dp_workflow:
        prompts = ctx.prompts or UiWorkflowPrompts()
        ctx.backup_dir_name = actions.edit_devinfo_persist(
            on_log=ctx.on_log,
            on_confirm=prompts.confirm,
            serialno=ctx.serialno,
        )


def _check_and_patch_arb(ctx: TaskContext) -> None:
    if ctx.tb320fc_arb_fallback:
        if not ctx.boot_target or not ctx.vbmeta_target:
            raise LTBoxError(get_string("wf_err_halted"))

        dumped_boot = const.BACKUP_DIR / f"{ctx.boot_target}.img"
        dumped_vbmeta = const.BACKUP_DIR / f"{ctx.vbmeta_target}.img"

        arb_status_result = actions.read_anti_rollback(
            dumped_boot_path=dumped_boot, dumped_vbmeta_path=dumped_vbmeta
        )

        if arb_status_result.status == ArbStatus.ERROR:
            raise LTBoxError(get_string("wf_step8_err_arb_abort"))

        if (
            ctx.modify_rollback_index == "ON"
            and arb_status_result.status == ArbStatus.MATCH
        ):
            arb_status_result = ArbResult(
                ArbStatus.NEEDS_PATCH,
                arb_status_result.boot_rollback,
                arb_status_result.vbmeta_rollback,
            )
    else:
        if ctx.device_rollback_index is None:
            raise LTBoxError(get_string("wf_err_halted"))

        arb_status_result = check_image_folder_arb(
            ctx.device_rollback_index, ctx.modify_rollback_index
        )

        if arb_status_result.status == ArbStatus.ERROR:
            raise LTBoxError(get_string("wf_step8_err_arb_abort"))

    if arb_status_result.status == ArbStatus.NEEDS_PATCH:
        ctx.arb_patched = True

    actions.patch_anti_rollback(comparison_result=arb_status_result)


def _flash_images(ctx: TaskContext) -> None:
    # Skip programmer loading if we just dumped partitions and stayed in EDL
    skip_reset_edl = not ctx.skip_dp_workflow or ctx.tb320fc_arb_fallback
    skip_dp = ctx.skip_dp_flash or (ctx.skip_dp_workflow and not ctx.use_backup_dp)

    actions.flash_full_firmware(
        dev=ctx.dev,
        skip_reset_edl=skip_reset_edl,
        skip_dp=skip_dp,
        wipe=bool(ctx.wipe),
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
        WorkflowStep(
            "wf_step4_convert_row"
            if ctx.target_region == "ROW"
            else "wf_step4_convert",
            lambda: _convert_region_images(ctx),
        ),
        WorkflowStep("wf_step5_modify_xml", lambda: _decrypt_and_modify_xml(ctx)),
        WorkflowStep(None, lambda: _detect_anti_rollback(ctx)),
        WorkflowStep(None, lambda: _check_backup_critical(ctx)),
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


def _log_target_region(ctx: "TaskContext") -> None:
    if not ctx.modify_region_code:
        key = "menu_main_install_keep" if ctx.wipe == 0 else "menu_main_install_wipe"
    elif ctx.target_region == "ROW":
        key = (
            "menu_main_install_keep_row"
            if ctx.wipe == 0
            else "menu_main_install_wipe_row"
        )
    else:
        key = (
            "menu_main_install_keep_prc"
            if ctx.wipe == 0
            else "menu_main_install_wipe_prc"
        )
    utils.ui.info(get_string(key))


def _build_success_result(ctx: TaskContext) -> TaskResult:
    success_msg = get_string("wf_process_complete")
    success_msg += f"\n{get_string('wf_process_complete_info')}"

    if ctx.backup_dir_name:
        success_msg += (
            f"\n\n{get_string('wf_backup_notice').format(dir=ctx.backup_dir_name)}"
        )

    if ctx.arb_patched:
        success_msg += f"\n\n{get_string('wf_arb_patched_warning')}"

    if ctx.modify_rollback_index == "OFF":
        success_msg += f"\n\n{get_string('wf_arb_off_notice')}"

    return TaskResult.from_message(success_msg)


def _run_patch_all(ctx: TaskContext) -> TaskResult:
    _log_target_region(ctx)

    _run_steps(ctx, _build_steps(ctx))
    return _build_success_result(ctx)


def patch_all(
    dev: device.DeviceController,
    wipe: Literal[0, 1] = 0,
    modify_region_code: bool = True,
    target_region: str = "PRC",
    modify_rollback_index: str = "ON",
    prompts: Optional[WorkflowPrompts] = None,
    manage_execution: bool = True,
) -> TaskResult:
    ctx = TaskContext(
        dev=dev,
        wipe=wipe,
        modify_rollback_index=modify_rollback_index,
        modify_region_code=modify_region_code,
        target_region=target_region,
        on_log=lambda s: utils.ui.info(s),
        prompts=prompts or UiWorkflowPrompts(),
    )

    log_file = (
        build_log_filename(const.BASE_DIR, "log_flash_firmware")
        if manage_execution
        else ""
    )
    command_name = "patch_all_wipe" if wipe == 1 else "patch_all"

    if manage_execution:
        announce_logging_start(
            command_name=command_name,
            log_file=log_file,
            info=utils.ui.info,
        )

    try:
        if manage_execution:
            with logging_context(log_file):
                return _run_patch_all(ctx)
        return _run_patch_all(ctx)

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
        if manage_execution:
            announce_logging_finished(log_file=log_file, info=utils.ui.info)
