import os
import shutil
from typing import List

from .logger import get_logger

logger = get_logger()


class ConsoleUI:
    def get_term_width(self, max_width: int = 78) -> int:
        return min(max_width, shutil.get_terminal_size((80, 24)).columns)

    def echo(self, message: str = "", err: bool = False) -> None:
        if err:
            logger.error(message)
        else:
            logger.info(message)

    def info(self, message: str) -> None:
        self.echo(message)

    def warn(self, message: str) -> None:
        self.echo(f"\033[93m{message}\033[0m", err=True)

    def error(self, message: str) -> None:
        self.echo(f"\033[91m{message}\033[0m", err=True)

    def banner(self, char: str = "=", indent: str = "  ") -> str:
        return f"{indent}{char * self.get_term_width()}"

    def box_output(self, lines: List[str], err: bool = False) -> None:
        self.echo("", err=err)
        for line in lines:
            self.echo(line, err=err)
        self.echo("", err=err)

    def prompt(self, message: str = "") -> str:
        return input(message)

    def clear(self) -> None:
        os.system("cls")


ui = ConsoleUI()
