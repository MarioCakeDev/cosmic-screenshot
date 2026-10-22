import pytest

from nllb_sidecar.__main__ import beam_size_from, min_confidence_from
from nllb_sidecar.engine import DEFAULT_BEAM_SIZE, MAX_BEAM_SIZE, EngineError
from nllb_sidecar.languages import LanguageTableError


def test_the_flag_wins_over_the_environment():
    assert min_confidence_from(0.2, {"NLLB_MIN_CONFIDENCE": "0.5"}) == 0.2


def test_the_environment_is_read_when_there_is_no_flag():
    assert min_confidence_from(None, {"NLLB_MIN_CONFIDENCE": "0.25"}) == 0.25


def test_no_setting_keeps_the_table_default():
    assert min_confidence_from(None, {}) is None
    assert min_confidence_from(None, {"NLLB_MIN_CONFIDENCE": "  "}) is None


def test_a_non_numeric_environment_value_is_one_clear_line():
    with pytest.raises(LanguageTableError) as error:
        min_confidence_from(None, {"NLLB_MIN_CONFIDENCE": "high"})
    assert "NLLB_MIN_CONFIDENCE='high' is not a number" in str(error.value)
    assert "\n" not in str(error.value)


@pytest.mark.parametrize("value", [-0.1, 1.5])
def test_out_of_range_is_rejected(value):
    with pytest.raises(LanguageTableError):
        min_confidence_from(value, {})


def test_the_beam_default_is_four_not_greedy():
    # Beam 1 invents whole sentences on isolated labels; 4 is the measured
    # compromise for CPU cost and label quality.
    assert DEFAULT_BEAM_SIZE == 4
    assert beam_size_from(None, {}) == 4


def test_the_beam_flag_wins_over_the_environment():
    assert beam_size_from(2, {"NLLB_BEAM_SIZE": "6"}) == 2


def test_the_beam_environment_is_read_when_there_is_no_flag():
    assert beam_size_from(None, {"NLLB_BEAM_SIZE": "3"}) == 3
    assert beam_size_from(None, {"NLLB_BEAM_SIZE": "  "}) == DEFAULT_BEAM_SIZE


def test_a_non_numeric_beam_is_one_clear_line():
    with pytest.raises(EngineError) as error:
        beam_size_from(None, {"NLLB_BEAM_SIZE": "wide"})
    message = str(error.value)
    assert "NLLB_BEAM_SIZE='wide' is not an integer" in message
    assert "\n" not in message


@pytest.mark.parametrize("value", [0, -1, MAX_BEAM_SIZE + 1])
def test_a_beam_outside_the_range_is_rejected(value):
    with pytest.raises(EngineError) as error:
        beam_size_from(value, {})
    assert f"between 1 and {MAX_BEAM_SIZE}" in str(error.value)
