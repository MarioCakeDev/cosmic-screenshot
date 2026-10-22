"""ISO 639-1 → NLLB-200 language tokens.

NLLB does not take a bare two-letter code: it needs the language *and* the
script (`deu_Latn`, `arb_Arab`, `zho_Hans`). The table lives in
`data/nllb_codes.json` and is the single source of truth for this sidecar; the
application's target picker is checked against the same file, and the engine
refuses to start if any code is missing from the loaded model's vocabulary.

Chinese is the one language whose script the detector cannot see: fastText
returns `zh` for both simplified and traditional text, so [`resolve_source`]
picks the script from the text itself.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Mapping

TABLE_VARIABLE = "NLLB_LANG_TABLE"
_DEFAULT_TABLE = Path(__file__).with_name("data") / "nllb_codes.json"

# fastText labels that are not the ISO 639-1 code this table is keyed by.
_LABEL_ALIASES = {
    "iw": "he",  # legacy Hebrew
    "in": "id",  # legacy Indonesian
    "jw": "jv",
    "mo": "ro",  # legacy Moldavian
    "no": "nb",
    "zh-cn": "zh",
    "zh-hans": "zh",
    "zh-sg": "zh",
    "zh-tw": "zh",
    "zh-hk": "zh",
    "zh-hant": "zh",
}

# Traditional/simplified character pairs, enough to tell a page of prose apart.
# Used only when the source language is Chinese, and only to pick the script.
_HAN_PAIRS = (
    "这這 们們 个個 说說 国國 会會 为為 对對 时時 来來 后後 学學 发發 里裡 "
    "么麼 开開 关關 过過 还還 让讓 应應 该該 与與 将將 没沒 现現 点點 电電 "
    "话話 见見 觉覺 样樣 东東 车車 门門 问問 间間 际際 业業 动動 员員 产產 "
    "实實 质質 进進 经經 济濟 义義 务務 华華 万萬 专專 长長 马馬 鸟鳥 鱼魚 "
    "龙龍 书書 张張 陈陳 刘劉 阳陽 阴陰 云雲 机機 构構 权權 术術 标標 种種 "
    "数數 据據 图圖 网網 络絡 线線 织織 绿綠 红紅 纪紀 级級 组組 统統 继繼 "
    "续續 练練 观觀 认認 识識 读讀 写寫 听聽 忆憶 语語 词詞 译譯 释釋 爱愛 "
    "亲親 妈媽 儿兒 岁歲 团團 队隊 齐齊 亿億 亚亞 农農 创創 劳勞 势勢 医醫 "
    "历歷 县縣 参參 变變 咏詠 响響 园園 围圍 坏壞 块塊 坚堅 压壓 丧喪 丰豐"
)
_TRADITIONAL = frozenset(pair[1] for pair in _HAN_PAIRS.split())
_SIMPLIFIED = frozenset(pair[0] for pair in _HAN_PAIRS.split())


class LanguageTableError(Exception):
    """The table is unusable: bad file, bad code, or code missing from the model."""


@dataclass(frozen=True)
class LanguageTable:
    languages: Mapping[str, str]
    default_source: str
    min_confidence: float

    @classmethod
    def load(cls, path: str | os.PathLike[str] | None = None) -> "LanguageTable":
        configured = path or os.environ.get(TABLE_VARIABLE) or _DEFAULT_TABLE
        source = Path(configured)
        try:
            raw = json.loads(source.read_text(encoding="utf-8"))
        except OSError as error:
            raise LanguageTableError(f"cannot read the NLLB language table {source}: {error}") from error
        except json.JSONDecodeError as error:
            raise LanguageTableError(f"the NLLB language table {source} is not valid JSON: {error}") from error

        languages = raw.get("languages")
        if not isinstance(languages, dict) or not languages:
            raise LanguageTableError(f"the NLLB language table {source} has no 'languages' object")
        for iso, code in languages.items():
            if not isinstance(code, str) or not _looks_like_nllb(code):
                raise LanguageTableError(
                    f"the NLLB language table maps '{iso}' to '{code}', which is not an NLLB code"
                )
        default_source = str(raw.get("default_source", "en")).lower()
        if default_source not in languages:
            raise LanguageTableError(
                f"the NLLB language table's default source '{default_source}' has no entry"
            )
        min_confidence = float(raw.get("min_confidence", 0.4))
        if not 0.0 <= min_confidence <= 1.0:
            raise LanguageTableError(f"min_confidence {min_confidence} is outside 0..1")
        return cls(dict(languages), default_source, min_confidence)

    def code(self, iso: str) -> str | None:
        """The NLLB token for `iso`, or `None` if this table cannot express it."""
        base = normalise_iso(iso)
        return self.languages.get(base)

    def require(self, iso: str) -> str:
        code = self.code(iso)
        if code is None:
            raise LanguageTableError(f"unknown language '{iso}'")
        return code

    def default_source_code(self) -> str:
        return self.require(self.default_source)

    def validate_against(self, vocabulary: Iterable[str]) -> None:
        """Fail loudly if a table entry is not a token of the loaded model."""
        vocabulary = set(vocabulary)
        missing = sorted({code for code in self.languages.values() if code not in vocabulary})
        if missing:
            shown = ", ".join(missing[:5])
            more = "" if len(missing) <= 5 else f" (+{len(missing) - 5} more)"
            raise LanguageTableError(
                f"the language table has {len(missing)} code(s) the model does not know: {shown}{more}"
            )


def normalise_iso(value: str) -> str:
    """`DE`, `de-DE`, `__label__zh-Hant` → `de`, `de`, `zh`."""
    label = value.strip().lower()
    if label.startswith("__label__"):
        label = label[len("__label__"):]
    base = label.replace("_", "-").split("-")[0]
    return _LABEL_ALIASES.get(label, _LABEL_ALIASES.get(base, base))


def resolve_source(table: LanguageTable, iso: str, text: str) -> str:
    """The NLLB source token for `iso` given the capture's text.

    Only Chinese is ambiguous: the detector reports `zh` for both scripts, so
    the script is read from the characters. Everything else comes straight from
    the table.
    """
    base = normalise_iso(iso)
    if base == "zh":
        return "zho_Hant" if looks_traditional(text) else "zho_Hans"
    return table.require(base)


def looks_traditional(text: str) -> bool:
    """True when traditional-only characters outnumber simplified-only ones."""
    traditional = sum(1 for char in text if char in _TRADITIONAL)
    simplified = sum(1 for char in text if char in _SIMPLIFIED)
    return traditional > simplified


def _looks_like_nllb(code: str) -> bool:
    language, _, script = code.partition("_")
    return (
        len(language) == 3
        and language.isalpha()
        and language.islower()
        and len(script) == 4
        and script.isalpha()
        and script[0].isupper()
        and script[1:].islower()
    )
