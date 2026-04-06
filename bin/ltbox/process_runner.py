from __future__ import annotations

import os
import re
import subprocess
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Optional, TypedDict, Union

from .logger import get_logger

logger = get_logger()
_TOOL_TIMESTAMP_PREFIX = re.compile(r"^\s*\d{2}:\d{2}:\d{2}:\s*")
_PROGRESS_PERCENT = re.compile(r"(\d+(?:\.\d+)?)\s*%")


def _normalize_stream_log_line(line: str) -> str:
    rendered = line.rstrip()
    if not rendered:
        return ""
    return _TOOL_TIMESTAMP_PREFIX.sub("", rendered, count=1)


@dataclass(frozen=True)
class RunOptions:
    capture: bool = False
    stream: bool = False
    check: bool = True
    cwd: Optional[Union[str, Path]] = None
    env: Optional[dict[str, str]] = None
    timeout: Optional[float] = None
    creationflags: int = 0


@dataclass(frozen=True)
class CommandResult:
    stdout: str
    stderr: str
    returncode: int
    combined_output: str


class SubprocessTextKwargs(TypedDict):
    encoding: str
    errors: str
    env: dict[str, str]
    cwd: Optional[Union[str, Path]]
    creationflags: int


def _get_subprocess_kwargs(
    env: dict[str, str], cwd: Optional[Union[str, Path]]
) -> SubprocessTextKwargs:
    run_env = env.copy()

    if cwd:
        resolved_cwd = str(Path(cwd).resolve())
        run_env["TMPDIR"] = resolved_cwd
        run_env["TEMP"] = resolved_cwd
        run_env["TMP"] = resolved_cwd

    return {
        "encoding": "utf-8",
        "errors": "ignore",
        "env": run_env,
        "cwd": cwd,
        "creationflags": 0,
    }


class CommandRunner:
    def run(
        self,
        command: Union[list[str], str],
        *,
        shell: bool = False,
        options: Optional[RunOptions] = None,
        on_output: Optional[Callable[[str], None]] = None,
    ) -> CommandResult:
        opts = options or RunOptions()
        run_env = opts.env if opts.env is not None else os.environ.copy()
        run_kwargs = _get_subprocess_kwargs(run_env, opts.cwd)

        if opts.capture:
            run_kwargs["creationflags"] = opts.creationflags
            proc = subprocess.run(
                command,
                shell=shell,
                check=False,
                capture_output=True,
                text=True,
                timeout=opts.timeout,
                **run_kwargs,
            )
            stdout = proc.stdout or ""
            stderr = proc.stderr or ""
            result = CommandResult(
                stdout=stdout,
                stderr=stderr,
                returncode=proc.returncode,
                combined_output=(f"{stderr}{stdout}" if stderr else stdout),
            )
            if opts.check and proc.returncode != 0:
                raise subprocess.CalledProcessError(
                    proc.returncode,
                    command,
                    output=stdout,
                    stderr=stderr,
                )
            return result

        if opts.stream:
            run_kwargs["creationflags"] = opts.creationflags
            process = subprocess.Popen(
                command,
                shell=shell,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                bufsize=1,
                **run_kwargs,
            )
            # When a timeout is set, start a watchdog that kills the process if
            # the entire stream read + wait exceeds the limit.  Without this,
            # a hung process that keeps stdout open would block indefinitely in
            # the ``for line in process.stdout`` loop below.
            watchdog: Optional[threading.Timer] = None
            if opts.timeout is not None:

                def _kill_on_timeout() -> None:
                    try:
                        process.kill()
                    except OSError:
                        pass

                watchdog = threading.Timer(opts.timeout, _kill_on_timeout)
                watchdog.start()

            output_lines: list[str] = []
            timed_out = False
            last_logged_pct = -10.0
            try:
                if process.stdout:
                    for line in process.stdout:
                        if on_output is not None:
                            on_output(line)
                        else:
                            m = _PROGRESS_PERCENT.search(line)
                            if m:
                                pct = float(m.group(1))
                                if pct - last_logged_pct >= 10.0 or pct >= 100.0:
                                    logger.info(_normalize_stream_log_line(line))
                                    last_logged_pct = pct
                            else:
                                logger.info(_normalize_stream_log_line(line))
                                last_logged_pct = -10.0
                        output_lines.append(line)

                process.wait(timeout=opts.timeout)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
                timed_out = True
            finally:
                if watchdog is not None:
                    watchdog.cancel()

            if timed_out:
                raise subprocess.TimeoutExpired(command, opts.timeout or 0.0)
            combined_output = "".join(output_lines)
            returncode = process.returncode
            if opts.check and returncode != 0:
                raise subprocess.CalledProcessError(
                    returncode,
                    command,
                    output=combined_output,
                )
            return CommandResult(
                stdout=combined_output,
                stderr="",
                returncode=returncode,
                combined_output=combined_output,
            )

        run_kwargs["creationflags"] = opts.creationflags
        proc = subprocess.run(
            command,
            shell=shell,
            check=False,
            text=True,
            timeout=opts.timeout,
            **run_kwargs,
        )
        if opts.check and proc.returncode != 0:
            raise subprocess.CalledProcessError(proc.returncode, command)
        return CommandResult(
            stdout="",
            stderr="",
            returncode=proc.returncode,
            combined_output="",
        )
