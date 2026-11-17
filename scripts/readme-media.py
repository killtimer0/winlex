"""Record and compose README media with FFmpeg."""
import argparse
from datetime import datetime, timezone
import json
import math
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]


def ffmpeg(arguments, log):
    with log.open('w', encoding='utf-8') as file:
        try:
            result = subprocess.run(['ffmpeg', '-hide_banner', '-y', '-filter_complex_threads', '1',
                                     *map(str, arguments)], stdout=subprocess.DEVNULL, stderr=file)
        except FileNotFoundError as error:
            raise RuntimeError('FFmpeg was not found on PATH') from error
    if result.returncode:
        raise RuntimeError('FFmpeg failed; see {}'.format(log))


def workflow_filter(recording):
    # GDI cursor masks can clear alpha despite valid, already-composited RGB.
    # The captured desktop is opaque; only our separate click rings use alpha.
    filters = ['[0:v]format=rgb24,setpts=PTS-STARTPTS[base0]']
    current = 'base0'
    scale = recording['dpi']/96
    half = math.ceil(29*scale)
    for index, event in enumerate(e for e in recording['events'] if e['kind'] == 'down'):
        start = event['time']
        age = '(T-{:.6f})/.3'.format(start)
        radius = '({:.3f}+{:.3f}*{})'.format(7*scale, 18*scale, age)
        alpha = ('if(between(T,{s:.6f},{e:.6f})*'
                 'lte(abs(hypot(X-{h},Y-{h})-{r}),{stroke:.3f}),'
                 '190*(1-{age}),0)').format(s=start, e=start+.3, h=half,
                                            r=radius, stroke=1.5*scale, age=age)
        filters.append("color=c=0x258cf4:s={size}x{size}:r=60,format=rgba,"
                       "geq=r=37:g=140:b=244:a='{alpha}'[ring{i}]".format(
                           size=half*2+1, alpha=alpha, i=index))
        following = 'base{}'.format(index+1)
        filters.append("[{base}][ring{i}]overlay=x={x}:y={y}:shortest=1:format=auto:"
                       "enable='between(t,{s:.6f},{e:.6f})'[{out}]".format(
                           base=current, i=index, x=event['x']-half, y=event['y']-half,
                           s=start, e=start+.3, out=following))
        current = following
    filters.append('[{}]scale=w=min(1000\\,iw):h=min(760\\,ih):'
                   'force_original_aspect_ratio=decrease:flags=lanczos,fps=50[out]'.format(current))
    return ';'.join(filters)


def generate(raw, output):
    recording = json.loads((raw / 'recording.json').read_text(encoding='utf-8'))
    # Expressions contain numbers only; file paths are separate command arguments.
    slope = 1/math.tan(math.radians(80))
    themes = ("[0:v][1:v]blend=all_expr='if(lte(X,W*.55+(H/2-Y)*{:.9f}),A,B)',"
              "scale=w=min(1200\\,iw):h=min(900\\,ih):"
              "force_original_aspect_ratio=decrease:flags=lanczos[out]").format(slope)
    output.mkdir(parents=True, exist_ok=True)
    png, gif = output / 'themes.pending.png', output / 'workflow.pending.gif'
    try:
        ffmpeg(['-i', raw / 'light.png', '-i', raw / 'dark.png', '-filter_complex', themes,
                '-map', '[out]', '-frames:v', '1', '-update', '1', png], raw / 'themes-compose.log')
        filters = workflow_filter(recording)
        (raw / 'workflow-filter.txt').write_text(filters, encoding='utf-8')
        ffmpeg(['-i', raw / 'workflow.mkv', '-filter_complex', filters, '-map', '[out]',
                '-c:v', 'ffv1', '-threads', '2', raw / 'workflow-effects.mkv'], raw / 'workflow-effects.log')
        ffmpeg(['-i', raw / 'workflow-effects.mkv', '-filter_complex',
                '[0:v]split[a][b];[a]palettegen=stats_mode=diff:reserve_transparent=0[p];'
                '[b][p]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle[out]',
                '-map', '[out]', '-loop', '0', gif], raw / 'workflow-encode.log')
        # Both encodes and recording cleanup succeed before replacing existing media.
        png.replace(output / 'themes.png')
        gif.replace(output / 'workflow.gif')
    finally:
        png.unlink(missing_ok=True)
        gif.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument('--config', type=Path, help='SSH GUI target configuration')
    source.add_argument('--local', action='store_true', help='record on this desktop')
    source.add_argument('--from-captures', type=Path, help='compose an existing record_media evidence directory')
    parser.add_argument('--exe', type=Path, help='existing Debug GUI executable (required for recording)')
    parser.add_argument('--winmd', type=Path, default=ROOT / 'inputs/Windows.Win32.winmd')
    parser.add_argument('--output', type=Path, required=True, help='generated PNG/GIF directory')
    args = parser.parse_args()
    if args.from_captures:
        if args.exe:
            parser.error('--from-captures does not accept --exe')
        generate(args.from_captures.resolve(), args.output.resolve())
        return
    if not args.exe:
        parser.error('recording requires --exe')
    evidence = (ROOT / 'tmp/readme-media' /
                 datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S%fZ')).resolve()
    print('Raw captures: {}'.format(evidence), flush=True)
    command = [sys.executable, str(ROOT / 'tests/gui/run.py')]
    command += ['--local', '--allow-local-desktop'] if args.local else ['--config', str(args.config)]
    command += ['--exe', str(args.exe), '--winmd', str(args.winmd), '--output', str(evidence),
                '--timeout', '180', '--', '--real-input', '--dpi-mode', 'pmv2', '--timeout', '120',
                '-o', 'python_files=readme_media.py', '-o', 'python_functions=record_media']
    completed = subprocess.run(command)
    if completed.returncode:
        raise RuntimeError('Recording failed; see {}'.format(evidence / 'pytest.log'))
    result = json.loads((evidence / 'result.json').read_text(encoding='utf-8'))
    cleanup = result['cleanup']
    if (result['exit_code'] or result['timed_out'] or cleanup['errors']
            or not all(cleanup[key] for key in ('pytest_exited', 'applications_exited', 'configs_removed'))):
        raise RuntimeError('Recording or cleanup did not complete; old media preserved')
    generate(evidence / 'record_media', args.output.resolve())
    print('Media: {}\nRaw captures: {}'.format(args.output.resolve(), evidence))


if __name__ == '__main__':
    main()
