import time
from threading import local
from contextlib import contextmanager
from typing import Dict, Generator, Optional

import requests  # type: ignore[import-untyped]

_SESSION_LOCAL = local()


def get_session() -> requests.Session:
    session = getattr(_SESSION_LOCAL, "session", None)
    if session is None:
        session = requests.Session()
        setattr(_SESSION_LOCAL, "session", session)
    return session


@contextmanager
def request_with_retries(
    method: str,
    url: str,
    *,
    headers: Optional[Dict[str, str]] = None,
    timeout: int = 30,
    retries: int = 3,
    backoff: float = 5,
    stream: bool = True,
    allow_redirects: bool = True,
) -> Generator[requests.Response, None, None]:
    session = get_session()
    for attempt in range(retries + 1):
        try:
            response = session.request(
                method,
                url,
                headers=headers,
                timeout=timeout,
                stream=stream,
                allow_redirects=allow_redirects,
            )
            response.raise_for_status()
            with response:
                yield response
            return
        except requests.RequestException:
            if attempt >= retries:
                raise
            time.sleep(backoff * (attempt + 1))
