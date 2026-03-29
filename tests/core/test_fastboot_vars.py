from ltbox.device_fastboot import _parse_getvar_all
from ltbox.actions.arb import compute_device_rollback_index


SAMPLE_GETVAR_ALL = """\
(bootloader) snapshot-update-status:none
(bootloader) stored_rollback_index:31 = 0
(bootloader) stored_rollback_index:30 = 0
(bootloader) stored_rollback_index:3 = 41B7A200
(bootloader) stored_rollback_index:2 = 41B7A200
(bootloader) stored_rollback_index:1 = 1
(bootloader) stored_rollback_index:0 = 0
(bootloader)current-slot:a
(bootloader) modelname:TB350XU
(bootloader) pserialno:9KKR42B7M583JN217QP030F
(bootloader)serialno:JN371R2K
(bootloader) product:lapis
"""


def test_parse_getvar_all_extracts_model():
    result = _parse_getvar_all(SAMPLE_GETVAR_ALL)
    assert result.model == "TB350XU"


def test_parse_getvar_all_extracts_slot():
    result = _parse_getvar_all(SAMPLE_GETVAR_ALL)
    assert result.slot_suffix == "_a"


def test_parse_getvar_all_extracts_serialno():
    result = _parse_getvar_all(SAMPLE_GETVAR_ALL)
    assert result.serialno == "JN371R2K"


def test_parse_getvar_all_extracts_stored_rollback_indices():
    result = _parse_getvar_all(SAMPLE_GETVAR_ALL)
    assert result.stored_rollback_indices[2] == 0x41B7A200
    assert result.stored_rollback_indices[3] == 0x41B7A200
    assert result.stored_rollback_indices[1] == 1
    assert result.stored_rollback_indices[0] == 0
    assert result.stored_rollback_indices[31] == 0


def test_parse_getvar_all_empty_output():
    result = _parse_getvar_all("")
    assert result.model is None
    assert result.slot_suffix is None
    assert result.serialno is None
    assert result.stored_rollback_indices == {}


def test_parse_getvar_all_no_stored_indices():
    output = "(bootloader) modelname:TB322FC\n(bootloader)current-slot:a\n"
    result = _parse_getvar_all(output)
    assert result.model == "TB322FC"
    assert result.slot_suffix == "_a"
    assert result.stored_rollback_indices == {}


def test_parse_getvar_all_pserialno_not_matched():
    output = "(bootloader) pserialno:9KKR42B7M583JN217QP030F\n(bootloader)serialno:JN371R2K\n"
    result = _parse_getvar_all(output)
    assert result.serialno == "JN371R2K"


def test_compute_device_rollback_index_with_valid_indices():
    indices = {0: 0, 1: 1, 2: 0x41B7A200, 3: 0x41B7A200}
    assert compute_device_rollback_index(indices) == 0x41B7A200


def test_compute_device_rollback_index_all_trivial():
    indices = {0: 0, 1: 1}
    assert compute_device_rollback_index(indices) is None


def test_compute_device_rollback_index_empty():
    assert compute_device_rollback_index({}) is None


def test_compute_device_rollback_index_picks_max():
    indices = {0: 0, 2: 0x10000, 3: 0x20000}
    assert compute_device_rollback_index(indices) == 0x20000


def test_compute_device_rollback_index_value_of_two_is_meaningful():
    indices = {0: 0, 1: 1, 2: 2}
    assert compute_device_rollback_index(indices) == 2
