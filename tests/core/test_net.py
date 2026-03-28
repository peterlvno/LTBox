from unittest.mock import MagicMock, patch

from ltbox import net


def test_request_with_retries_uses_thread_local_session():
    session = MagicMock()
    response = MagicMock()
    response.raise_for_status.return_value = None
    response.__enter__.return_value = response
    response.__exit__.return_value = False
    session.request.return_value = response

    with patch("ltbox.net.get_session", return_value=session):
        with net.request_with_retries("GET", "https://example.com") as result:
            assert result is response

    session.request.assert_called_once_with(
        "GET",
        "https://example.com",
        headers=None,
        timeout=30,
        stream=True,
        allow_redirects=True,
    )


def test_get_session_reuses_session_within_thread():
    with patch("ltbox.net.requests.Session") as session_factory:
        session_factory.return_value = MagicMock()
        net._SESSION_LOCAL = type(net._SESSION_LOCAL)()

        first = net.get_session()
        second = net.get_session()

    assert first is second
    session_factory.assert_called_once_with()
