# Bundled plotting data

- `coastlines.txt`: Natural Earth 1:110m coastlines, repository release v5.1.2. Public domain. Source: https://raw.githubusercontent.com/nvkelso/natural-earth-vector/v5.1.2/geojson/ne_110m_coastline.geojson . Terms: https://www.naturalearthdata.com/about/terms-of-use/ . Each line is one source polyline, as longitude,latitude pairs rounded to six decimal places. Coordinates within numerical rounding of the date line are clamped to ±180°. No features were omitted or simplified. These are present-day coastlines.
- `viridis.rgb`, `plasma.rgb`, `cividis.rgb`: the 256 published RGB samples from Matplotlib v3.10.7 `lib/matplotlib/_cm_listed.py`, rounded to 8-bit RGB.
- `blue_red.rgb`: Matplotlib v3.10.7 `lib/matplotlib/_cm.py` ColorBrewer RdBu samples, reversed so blue is low and red is high, rounded to 8-bit RGB. The app interpolates between these samples.
- Matplotlib source: https://github.com/matplotlib/matplotlib/tree/v3.10.7 . Its license is included in `LICENSE_MATPLOTLIB`.

The app embeds these files and does not download data at runtime.

- `Inter-Regular.otf` and `Inter-SemiBold.otf`: unmodified fonts from the official Inter 4.1 release, https://github.com/rsms/inter/releases/tag/v4.1 . Copyright the Inter Project Authors; SIL Open Font License 1.1, included as `LICENSE_INTER`. Used in the interface and exported figures.
