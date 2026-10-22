import pytest

from nllb_sidecar.__main__ import min_confidence_from
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
