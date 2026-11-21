# Automaton Worm

Watch five-cell worms forage in a cellular automaton. Off, Direct, and Wave run on
the same seed and tick so you can compare food pickups, blocked rounds, and
recovery times. Each cell reads four neighbors from the previous tick. Worms use
space tickets to move, and cells spread and fade food scent. In Direct mode, a
stuck worm can ask a neighbor to make room. In Wave mode, a blocked recipient can
pass that request on.

The optimized integer MLP is the default. It is programmed from the same rule
table as the compiled table engine. Switch engines in Settings to compare them
during the same run.

Try the [live demo](https://quietmaintainer.github.io/automaton-worm/) in your browser.

## Run locally

For a checkout with a built `dist/`, serve it from the project directory.

```sh
python -m http.server 8080 --bind 127.0.0.1 --directory dist
```

Open [the local demo](http://127.0.0.1:8080/). You can also extract a tagged
release ZIP into `dist/` and use the same command. On hosts with a `python3`
command, use it in place of `python`.

To build from source, install [Rustup](https://rustup.rs/) and Python 3.9 or
later, then run these commands from the project directory.

```sh
rustup target add wasm32-unknown-unknown
cargo install --locked wasm-bindgen-cli --version 0.2.130
python scripts/build-web.py
```

The project pins Rust 1.96.0. The build script produces the static site with
the matching Wasm bindings.

### Docker

```sh
docker build -t automaton-worm .
docker run --rm -p 127.0.0.1:8080:80 automaton-worm
```

Open [the local demo](http://127.0.0.1:8080/). Once a container release is
public, replace the image in the run command with
`ghcr.io/OWNER/automaton-worm:v0.1.0` using the lowercase GitHub owner name and
the release tag.

If the Docker build runs behind a TLS-inspecting proxy, pass the trusted
CA in PEM format with
`docker build --secret id=build_ca,src=/path/to/ca.pem -t automaton-worm .`.
The certificate is used by the build stage. The nginx image receives the
static site.

## Controls

- **Play/Pause**, **Step** one tick, or **Reset** to the current seed. The first
  run opens paused on a 100×100 grid with 100 worms and 32 food pellets.
- Open **Settings** to pause and choose an engine, the displayed arena, a new
  seed, population, zoom, or overlays. The grid grows as needed for up to 1,000
  worms. A round takes 24 ticks. Max starts the next round as soon as the worker
  results arrive.
- Select a cell to see its memory and port messages. Expand **Cell rules** to
  view the rule rows. Use the grid's arrow keys to move the selection. Scroll
  to zoom and drag to pan while zoomed.
- Settings are saved in the browser. Each reload starts the saved seed at tick
  zero.

### 1D Worm Automata

Choose **1D Worm Automata** under **Settings → Mode**. The browser arena has a
350×350 grid, 10,000 five-cell worms, and 300 starting food pellets.

Click **16-rule sequence** to loop through rules 30, 54, 60, 90, 94, 110,
122, 126, 150, 158, 182, 188, 190, 220, 222, and 250. Each rule runs for five
seconds. Between rules, formation cues clear for one second while the worms
keep moving on the same grid. Pause freezes the sequence timer along with the
simulation. Click the sequence button again to release the current formation.

Each target is a 5×5 tile that can be filled by five worms. Use **Start a rule**
in Settings to start one rule, or turn on **Show formation signals** to see
where vacancies are advertised and which lanes are reserved.

## Checks

```sh
rustup component add rustfmt clippy
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --target wasm32-unknown-unknown --lib -- -D warnings
cargo test --locked
python scripts/check-replays.py
```

The replay check compares all cell words and observer metrics between the two
engines on seeds 412001–412064 at 1,000 worms and 32 pellets. It also checks
switches after scored ticks 17 and 59. SHA-256 fixtures were recorded from the
original compiled table for Off, Direct, and Wave. Even seeds have 288 warm-up
ticks followed by the 100 scored ticks. Odd seeds start scoring immediately.

### Max playback measurement

In headless Chrome 157 on Windows, each timed run advanced all three arenas
through 240 ticks, in 24-tick batches, with 1,000 worms and 32 pellets per
arena. Across the same 64 seeds, the optimized MLP took a median of 50.9 ms
and the compiled table took 105.1 ms. The median paired speedup was 2.08×
and the MLP was faster on all 64 seeds. Timing covers worker coordination
and frame transfer. Canvas painting, initialization, and warm-up are outside
the timed region.

Run the [browser benchmark](scripts/benchmark-browser.js) against the local
preview with Playwright CLI.

~~~sh
npx --package @playwright/cli@0.1.24 playwright-cli -s=worm-bench open http://127.0.0.1:8080/ --browser=chrome
npx --package @playwright/cli@0.1.24 playwright-cli -s=worm-bench run-code --filename=scripts/benchmark-browser.js --raw
npx --package @playwright/cli@0.1.24 playwright-cli -s=worm-bench close
~~~

## Deploy and release

GitHub Actions builds and deploys the demo from main. To host a fork, choose
**GitHub Actions** in **Settings → Pages**, then run the **GitHub Pages**
workflow. The live site uses the repository name as its URL path.

A version tag such as v0.1.0 triggers the **Release** workflow. It publishes a
static ZIP under GitHub Releases and an amd64/arm64 image in GHCR. To allow
anonymous pulls, set the GHCR package to public.

[Pages deployment](https://docs.github.com/en/pages/getting-started-with-github-pages/using-custom-workflows-with-github-pages) ·
[Container registry](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry)

## License

Automaton Worm is distributed under the [MIT license](LICENSE). See [NOTICE](NOTICE)
for source credits and [THIRD_PARTY_NOTICES](THIRD_PARTY_NOTICES) for the Wasm dependencies.
