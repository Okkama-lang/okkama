"""Figure 1.1: the Bayesian Occam factor (after MacKay 2003, ch. 28).

A simple model spreads its predictions over a narrow range of possible data,
a flexible one over a wide range. Both distributions integrate to one, so
where the data fall in the narrow range the simple model is more probable.
"""
import math
from svg import Plot, SERIES, text

for language in ("en", "ru"):
    plot = Plot()
    plot.frame((0, 10), (0, 0.45))
    simple = [(x / 20, math.exp(-((x / 20 - 3.5) ** 2) / (2 * 1.0 ** 2)) / math.sqrt(2 * math.pi)) for x in range(0, 201)]
    flexible = [(x / 20, math.exp(-((x / 20 - 5) ** 2) / (2 * 2.6 ** 2)) / (2.6 * math.sqrt(2 * math.pi))) for x in range(0, 201)]
    plot.band(2.6, 4.4, SERIES[2])
    plot.area(simple, SERIES[0])
    plot.area(flexible, SERIES[1])
    plot.line(simple, SERIES[0])
    plot.line(flexible, SERIES[1], dash="6 4")
    plot.axes(text(("possible data sets", "возможные наборы данных"), language),
              text(("P(data | model)", "P(данные | модель)"), language),
              xticks=[(3.5, "C₁")])
    plot.label(3.6, 0.41, text(("H₁: simple model", "H₁: простая модель"), language), SERIES[0])
    plot.label(7.2, 0.17, text(("H₂: flexible model", "H₂: гибкая модель"), language), SERIES[1])
    plot.label(3.5, 0.02, text(("observed", "наблюдаемые"), language), SERIES[2], anchor="middle")
    plot.save("occam-evidence", language)
