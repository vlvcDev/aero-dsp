"""Small, original harmonic recipe index; no network, embeddings or copied songs.

These are creative starting points, not rules defining a genre. Patterns use
Roman numerals relative to the requested tonic. Text is project-authored and
distributed under the repository license.
"""
import re

RECIPES = [
    ('pop', 'pop bright hopeful uplifting happy', 'I–V–vi–IV; vi–IV–I–V',
     'Contrast major stability with relative-minor color; change the opening or cadence to avoid a stock loop.',
     'Usually 2 or 4 beats per chord; a longer final chord can establish arrival.'),
    ('indie', 'indie melancholy wistful bittersweet reflective sad', 'I–iii–IV–iv; i–VI–III–VII',
     'A borrowed minor iv can add bittersweet color in major. Choose major or minor deliberately, not just by mood label.',
     'Try a held opening chord and quicker approach to the final chord.'),
    ('jazz', 'jazz lounge warm romantic sophisticated neo soul rnb', 'ii7–V7–Imaj7; Imaj7–vi7–ii7–V7',
     'Use sevenths or supported ninth qualities and clear resolution; favor common tones. Do not invent altered chords outside the supported library.',
     'Two beats for ii and V, four for tonic is one option; steady four-beat changes also work.'),
    ('ambient', 'ambient dreamy floating spacious shoegaze ethereal peaceful', 'I(sus2)–IVmaj7–vi7–Vsus4',
     'Suspensions and shared ringing tones can soften cadences. Leave space instead of adding chords just for complexity.',
     'Four or eight beats per chord can support a spacious texture.'),
    ('metal', 'metal thrash heavy aggressive dark tense rock punk', 'i5–bII5–bVI5–V5; i5–bVII5–bVI5–bVII5',
     'Power chords avoid dense thirds under distortion. Chromatic neighbors can create tension; retain a recognizable tonic.',
     'Mix one- or two-beat changes with held tonic chords; this describes harmony, not a picking pattern.'),
    ('folk', 'folk country acoustic roots gentle singer songwriter', 'I–IV–I–V7; I–vi–IV–V',
     'Simple triads and available open shapes support singing. Prefer a clear cadence and avoid unnecessary extensions.',
     'Two or four beats per chord; hold the final tonic when resolution is wanted.'),
    ('blues', 'blues soulful gritty shuffle', 'I7 I7 I7 I7 / IV7 IV7 I7 I7 / V7 IV7 I7 V7',
     'Dominant sevenths on I, IV and V are intentional blues color, not errors to force into a major scale.',
     'The example is twelve bars, four beats each; harmonic duration alone does not create a shuffle groove.'),
    ('cinematic', 'cinematic suspense mysterious dramatic ominous epic', 'i–iv–bVI–V; i–bVI–III–VII',
     'A major V can give minor-key tension and resolution. Choose whether the ending resolves or leaves a loop open.',
     'Long holds followed by shorter changes can build tension.'),
    ('funk', 'funk funky disco rhythmic dance', 'i7–IV7; I7–IV7',
     'A small harmonic vocabulary can support a strong groove. The chord sequence alone cannot specify syncopated strumming.',
     'Use held one- or two-bar harmony, or balanced short changes rather than constant harmonic novelty.'),
]


def retrieve_guidance(prompt, limit=3):
    words = set(re.findall(r'[a-z0-9]+', prompt.lower()))
    ranked = sorted(RECIPES, key=lambda r: -len(words.intersection(r[1].split())))
    return [{'id': r[0], 'tags': r[1], 'patterns': r[2], 'harmony': r[3],
             'timing': r[4], 'source': 'AERO DSP original recipe',
             'license': 'Repository license'}
            for r in ranked if words.intersection(r[1].split())][:limit]
