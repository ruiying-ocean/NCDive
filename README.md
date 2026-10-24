# NCDive

An independent Rust desktop viewer for NetCDF files, inspired by NASA Panoply.

Open local NetCDF classic and NetCDF-4 files to explore:

- Searchable variables, including variables in nested groups.
- Variable types, shapes, dimensions and units.
- Global, group and variable attributes.
- Unlimited dimensions and the stored metadata for missing values and calendars.
- Equal Earth (default), Equirectangular, Mollweide and Robinson maps, with time/depth (and other extra-dimension) selectors.
- Viridis, Plasma, Cividis and blue–white–red palettes, with linear or Log₁₀ scales and adjustable colour limits.
- Optional present-day coastlines, latitude–depth and longitude–depth sections, and current-minus-reference difference plots.
- PNG and PDF figure export, with the selected scale, units, source paths, slice coordinates and missing-value notes.
- Optional cell inspection, with hover values and keyboard-operable row/column controls.

The interface and exported figures use bundled Inter fonts, with regular text, semibold headings, and larger small labels.

Files are opened read-only. Metadata and selected two-dimensional slices load in the background. Select a variable, then use **Plot** or **Metadata** above its details.

## Maps

Maps support independent one-dimensional latitude and longitude coordinates, including unevenly spaced cGENIE grids and descending axes. Cells use CF `bounds` or cGENIE `edges` when available; otherwise boundaries are inferred from coordinate midpoints. For cGENIE staggered grids with samples exactly at the poles, extrapolated outer `edges` are clipped to ±90° while keeping the pole samples. Other invalid declared bounds are rejected. Longitudes are displayed from −180° to 180°, with cells split at the date line.

Maps also accept two-dimensional latitude/longitude arrays, including NEMO `nav_lon(y,x)` / `nav_lat(y,x)` and other CF curvilinear grids. Coordinates are read from the variable's `coordinates` attribute, with conventional coordinate names used when it is absent. Paired [CF bounds](https://cfconventions.org/cf-conventions/cf-conventions.pdf) (`row,column,4`) supply cell corners; otherwise corners are estimated from neighbouring coordinates on the sphere and marked as approximate in the plot and exports. Both coordinate arrays must be finite, share the same dimensions and order, and use degrees. Inferred grids hide coincident cells, preferring interior cells over duplicate halos, so unused NEMO boundary values do not paint over the ocean. Stored values, counts and row/column indices are preserved. Time/depth selection, all four projections, inspection, differences, and exports work on these maps. Vertical sections through curvilinear grids are not yet supported.

Maps start in **Equal Earth**, which preserves the relative sizes of regions. Use **Projection** above the map to switch to **Equirectangular (lat–lon)**, **Mollweide** (equal area), or **Robinson** (a compromise world view). These are spherical display projections centred on 0° longitude. The selected projection stays in place when changing variables or slices; opening a new dataset restores Equal Earth. Grid cells, grid lines and the cell inspector all follow the projection. Regular-grid edges use latitude strips no wider than 1°. Curvilinear cell edges are subdivided to at most 1° and refined further where the projection bends sharply near the poles. Cell values are not smoothed or interpolated. Mollweide hides longitude labels because its meridians meet at the poles. In the other projections, longitude labels sit below the map, with guide lines to their meridians; narrow maps show fewer labels to prevent crowding. The blank area outside the world outline has no data cells.

Missing values (including default fill values), non-finite values and values outside the declared valid range are masked before applying `scale_factor` and `add_offset`. If a file uses placeholders that its metadata does not declare, enter them under **Missing values** and click **Apply**. For these NEMO snapshots, use `-1e35, -9999`; add `0` only when zero means missing for your field. Entries match the variable's stored precision and are masked before scaling or subtracting a reference. Applying changes also resets the colour limits. The list stays in place across variables and slices of the open file, applies to both inputs of a comparison, and is recorded in figure notes. Clear the box and apply to return to metadata-only masking. Opening another file resets the list; source files are never modified. Gray means missing/masked, which is not necessarily land. Colour limits stay fixed when changing slices; **Auto range** resets them to the current slice. Enable **Inspect cells** beside the palette and scale controls when you need exact values. It is off by default; when enabled, it shows hover values and row/column controls for keyboard access.

Use **Palette** and **Scale** above the plot to choose colours and linear or Log₁₀ scaling. Log scales use positive colour limits; zero and negative values are shown in tan and counted separately from gray missing values. The cell inspector still reports their original values. Changing the scale type fits the limits to the current slice.

Enable **Modern coastlines** to add bundled Natural Earth 1:110m outlines. They are present-day coastlines and may not match a model's land mask or palaeogeography. No data is downloaded while the app runs. Sources and licences are recorded in [assets/README.md](assets/README.md).

## Sections and differences

Selecting a variable keeps the current view when its coordinates support it; otherwise the app chooses a compatible map or section automatically. For example, cGENIE `phys_opsia` opens as a latitude–depth section.

Choose **Latitude–depth section** or **Longitude–depth section** from **View**. The remaining coordinate and time controls select the transect. Sections also accept one-dimensional height or pressure coordinates, label their stored units, and follow the coordinate's `positive` direction. Without that attribute, pressure increases downward; nonnegative depth/`zt` coordinates also increase downward. Dimensionless, terrain-following and curvilinear vertical coordinates are not supported.

Use **Reference file…** to select the field to subtract. You can choose another run, or the current file to compare variables or slices. Select the reference variable and set its slice independently. The result is **current − reference**; both source paths and selections are displayed or included in exports. Comparisons require matching coordinates, cell boundaries and stored grid order, with matching nonempty unit strings. Equivalent longitude wraps are accepted; regridding and unit conversion are not performed. A cell is missing if either input is missing. New comparisons use the blue–white–red palette and limits centred on zero. **Remove comparison** returns to the original field. Selecting a different primary variable clears its comparison.

## Figure export

**File → Export PNG…** and **File → Export PDF…** export the displayed map or section without the application controls or hover highlight. PNG figures are 1200 pixels wide; PDFs preserve vector cells, lines and embedded text. Figures include palette/scale, units, full source paths, slice coordinates, comparison order, and missing/clipped-value notes. Export runs in the background and reports success or failure in the bottom status bar, even after you switch to another variable. On macOS, File is in the system menu bar at the top of the screen; other platforms use a menu inside the window. Export commands are enabled when a valid plot is selected. Source NetCDF files are never written.

Time/depth labels show the coordinates and units stored in the file. Calendar dates are not converted; model time labels should not be assumed to be calendar years.

Supported map storage types are floating point and integers up to 32 bits. Projected input grids without geographic latitude/longitude arrays, unstructured or time-varying grids, `_Unsigned` packing, 64-bit integer fields, line plots, polar views, custom projection settings, contour lines and animations are not implemented yet. Other variables remain available in the metadata browser.

Real-file compatibility checks use the public [xarray example datasets](https://github.com/pydata/xarray-data): NCEP air-temperature maps and ERA-Interim wind/geopotential maps and pressure sections. Selected slices are compared cell by cell with an independent decoder. The RASM example also plots using its two-dimensional latitude and longitude coordinates; all four projections and cell inspection have been checked. These checks establish support for those files and layouts, not every file from those models.

NEMO checks use local ORCA2 snapshots for 2000 and 2100 from the `diad_T`, `ptrc_T` and `limphy` families (149 × 182 cells). Surface and deep `TChl` and surface `PPINT` slices match an independent netCDF4 decoder cell by cell. These checks also cover time/depth selection, matching-grid differences, cell inspection, date-line/polar geometry, duplicate halos, and PNG/PDF output in all four projections. Files without declared cell corners still use approximate boundaries.

File access uses [GeoRust's netcdf crate](https://github.com/georust/netcdf); projections use [GeoRust's proj bindings](https://github.com/georust/proj) to PROJ. Metadata handling follows the [CF conventions](https://cfconventions.org/cf-conventions/cf-conventions.html).

## Run

Install [Rust](https://rust-lang.org/tools/install/), the NetCDF C library, and PROJ 9.6 or newer. On macOS with Homebrew:

```sh
brew install netcdf proj pkg-config
```

From this folder:

```sh
source "$HOME/.cargo/env"
cargo run --release -- samples/temperature.nc
```

Run `cargo run --release` without a file to start with an empty window. Drag a NetCDF file into the Explore window, or use **File → Open NetCDF file…** or **Cmd+O** (**Ctrl+O** on other platforms) to choose a file. Drop one file at a time; you can also drop another file to replace the current dataset once loading finishes. Select a variable in the sidebar to see its details, or **Dataset overview** for global and group metadata.

Use optimized release builds for everyday viewing, especially large NEMO grids. Desktop app bundles should use the executable from `cargo build --release` at `target/release/ncdive`.

On macOS, package the release executable as `NCDive.app/Contents/MacOS/ncdive` with `macos/Info.plist` copied to `NCDive.app/Contents/Info.plist`. The bundle supports opening `.nc`, `.nc4` and `.cdf` datasets directly from Finder, including when the app is already running. Install it in `~/Applications`, then choose it under a NetCDF file's **Get Info → Open with → Change All…** to make it the default. Open one dataset at a time.

The app links to native NetCDF, HDF5 and PROJ libraries. If Cargo cannot find a custom installation, set `NETCDF_DIR` and `HDF5_DIR` to their respective installation prefixes; ensure PROJ's `lib/pkgconfig` directory is on `PKG_CONFIG_PATH`. Machine-specific overrides can go in `.cargo/config.toml`, which is ignored by Git. Distributing a standalone app will require bundling its native dependencies.

NCDive is designed for macOS, Windows and Linux, using a shared Rust interface and platform-specific file menus. Only macOS has been built and tested so far. Windows and Linux still need native-library setup, build and runtime checks, and app packaging/file associations before they can be considered supported releases.

## Check

```sh
cargo test
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

The sample files contain synthetic data. Their readable CDL sources are included; regenerate them with the `ncgen` tool provided by NetCDF:

```sh
ncgen -k classic -o samples/temperature.nc samples/temperature.cdl
ncgen -k netCDF-4 -o samples/groups.nc samples/groups.cdl
ncgen -k netCDF-4 -o samples/packed.nc samples/packed.cdl
ncgen -k classic -o samples/sections.nc samples/sections.cdl
ncgen -k classic -o samples/overturning.nc samples/overturning.cdl
ncgen -k classic -o samples/reanalysis.nc samples/reanalysis.cdl
ncgen -k netCDF-4 -o samples/curvilinear.nc samples/curvilinear.cdl
```

The tests cover metadata, read-only behavior, invalid files, nested groups, slicing, packed values, missing/default-fill values, mismatched NaN fill-attribute types, pressure direction, reversed latitude, dimension order, explicit and inferred cell bounds, date-line wrapping, two-dimensional coordinates and CF corners, folded cells and duplicate halos, constant/all-missing maps, projection reference positions, inverse cell inspection, projected grid geometry, palettes and log scales, both section orientations, comparison validation, coastline date-line handling, and PNG/PDF rendering. Export checks write synthetic preview figures to `target/export-previews/`.


To rerun the optional real NEMO check with the snapshot collection above:

```sh
NCDIVE_NEMO_SNAPSHOTS=/path/to/snapshots cargo test nemo_snapshots -- --ignored
```

It reads the snapshots without modifying them and writes preview figures and decoded comparison slices to `target/export-previews/`.

To measure mouse-driven drawing and scrolling on the same snapshots, run `NCDIVE_NEMO_SNAPSHOTS=/path/to/snapshots cargo test --release nemo_interaction_timings -- --ignored --nocapture`. The timings cover cell lookup, CPU drawing work and scroll-area redraws; they exclude display refresh and GPU presentation.
