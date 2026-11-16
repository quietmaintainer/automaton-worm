const boardBackground = '#0e1620';
const wormColors = ['#4dd9c2', '#bd9dfa', '#fa8aa3', '#81b8ff'];
const foodColor = '#ffc56d';
const roomColors = { request: '#ffdd8a', forwarded: '#6be8f4', yielded: '#a4f18c' };
const headingOffsets = [[-1, 0], [1, 0], [0, -1], [0, 1]];
const WORM_LENGTH = 5;

function gridPath(size) {
  const path = new Path2D();
  for (let i = 0; i <= size; i++) {
    path.moveTo(i, 0);
    path.lineTo(i, size);
    path.moveTo(0, i);
    path.lineTo(size, i);
  }
  return path;
}

function scentLayer() {
  const canvas = document.createElement('canvas');
  const context = canvas.getContext('2d');
  let previous;
  let pixels;
  return (values, size) => {
    if (previous === values) return canvas;
    if (canvas.width !== size || canvas.height !== size) {
      canvas.width = canvas.height = size;
      pixels = context.createImageData(size, size);
    }
    for (let cell = 0; cell < values.length; cell++) {
      pixels.data[cell * 4] = 255;
      pixels.data[cell * 4 + 1] = 197;
      pixels.data[cell * 4 + 2] = 109;
      pixels.data[cell * 4 + 3] = values[cell] ? 8 + 2 * values[cell] : 0;
    }
    context.putImageData(pixels, 0, 0);
    previous = values;
    return canvas;
  };
}

// Keep segments connected visually when the opposite end takes the lead.
function wormStepSources(bodies, previous) {
  const sources = new Uint32Array(bodies.length);
  for (let start = 0; start < bodies.length; start += WORM_LENGTH) {
    const tail = start + WORM_LENGTH - 1;
    const reversed = bodies[start + 1] === previous[tail];
    const offset = reversed ? tail : start;
    const stride = reversed ? -1 : 1;
    let advances = bodies[start] !== previous[start];
    for (let segment = 1; segment < WORM_LENGTH && advances; segment++) {
      advances = bodies[start + segment] === previous[offset + (segment - 1) * stride];
    }
    for (let segment = 0; segment < WORM_LENGTH; segment++) {
      sources[start + segment] = advances ? previous[offset + segment * stride] : bodies[start + segment];
    }
  }
  return sources;
}

function circle(path, x, y, radius) {
  path.moveTo(x + radius, y);
  path.arc(x, y, radius, 0, 2 * Math.PI);
}

function drawPellets(context, pellets, size) {
  const path = new Path2D();
  for (const cell of pellets) {
    const x = cell % size + .5;
    const y = Math.floor(cell / size) + .5;
    path.moveTo(x, y - .42);
    path.lineTo(x + .42, y);
    path.lineTo(x, y + .42);
    path.lineTo(x - .42, y);
    path.closePath();
  }
  context.fillStyle = foodColor;
  context.fill(path);
}

function drawWorms(context, { bodies, headings, yieldedWorms }, size, sources, progress, bounds) {
  const chains = wormColors.map(() => new Path2D());
  const segments = wormColors.map(() => new Path2D());
  const centers = new Path2D();
  const yieldLines = new Path2D();
  const yieldHeads = new Path2D();
  const yielding = new Set(yieldedWorms);
  for (let start = 0; start < bodies.length; start += WORM_LENGTH) {
    const worm = start / WORM_LENGTH;
    const color = worm % wormColors.length;
    const headX = bodies[start] % size;
    const headY = Math.floor(bodies[start] / size);
    if (headX < bounds.left - WORM_LENGTH || headX > bounds.right + WORM_LENGTH
      || headY < bounds.top - WORM_LENGTH || headY > bounds.bottom + WORM_LENGTH) continue;
    for (let part = 0; part < WORM_LENGTH; part++) {
      const cell = bodies[start + part];
      const prior = sources?.[start + part] ?? cell;
      const x = .5 + (prior % size) * (1 - progress) + (cell % size) * progress;
      const y = .5 + Math.floor(prior / size) * (1 - progress) + Math.floor(cell / size) * progress;
      if (part === 0) {
        chains[color].moveTo(x, y);
        if (yielding.has(worm)) {
          yieldLines.moveTo(x, y);
          circle(yieldHeads, x, y, .55);
        }
        const [dx, dy] = headingOffsets[headings[worm]];
        circle(segments[color], x, y, .43);
        circle(centers, x + dx * .23, y + dy * .23, .105);
      } else {
        chains[color].lineTo(x, y);
        if (yielding.has(worm)) yieldLines.lineTo(x, y);
        circle(segments[color], x, y, .31);
        circle(centers, x, y, .13);
      }
    }
  }
  context.lineCap = 'round';
  context.lineJoin = 'round';
  if (yielding.size) {
    context.strokeStyle = context.fillStyle = roomColors.yielded;
    context.lineWidth = .9;
    context.stroke(yieldLines);
    context.fill(yieldHeads);
  }
  context.lineWidth = .34;
  for (const [index, color] of wormColors.entries()) {
    context.strokeStyle = context.fillStyle = color;
    context.stroke(chains[index]);
    context.fill(segments[index]);
  }
  context.fillStyle = boardBackground;
  context.fill(centers);
}

function drawRoomSignals(context, signals, size, unit, color, pixel) {
  if (!signals?.length) return;
  context.beginPath();
  for (let index = 0; index < signals.length; index += 2) {
    const from = signals[index];
    const to = signals[index + 1];
    const fromX = from % size + .5;
    const fromY = Math.floor(from / size) + .5;
    const dx = to % size + .5 - fromX;
    const dy = Math.floor(to / size) + .5 - fromY;
    // Offset each arrow so requests in opposite directions remain visible.
    const x = fromX - dy * .17;
    const y = fromY + dx * .17;
    const tipX = x + dx * .77;
    const tipY = y + dy * .77;
    const wingX = tipX - dx * .26;
    const wingY = tipY - dy * .26;
    context.moveTo(x + dx * .18, y + dy * .18);
    context.lineTo(tipX, tipY);
    context.moveTo(wingX - dy * .19, wingY + dx * .19);
    context.lineTo(tipX, tipY);
    context.lineTo(wingX + dy * .19, wingY - dx * .19);
  }
  const stroke = Math.min(.26, Math.max(.11, 1.2 * pixel / unit));
  context.strokeStyle = boardBackground;
  context.lineWidth = stroke + .14;
  context.stroke();
  context.strokeStyle = color;
  context.lineWidth = stroke;
  context.stroke();
}

export function connectView(canvas, { onSelect, onZoom }) {
  const context = canvas.getContext('2d', { alpha: false });
  const scent = scentLayer();
  const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');
  let size = 0;
  let grid;
  let frame;
  let centerX = .5;
  let centerY = .5;
  let zoom = 100;
  let selection = null;
  let animation;
  let pendingPaint = 0;
  let drag;

  function transform() {
    const unit = Math.max(1, Math.min(canvas.width, canvas.height) - 2) * zoom / (100 * size);
    return {
      unit,
      x: canvas.width / 2 - centerX * size * unit,
      y: canvas.height / 2 - centerY * size * unit,
    };
  }

  function paint(now) {
    pendingPaint = 0;
    const pixel = window.devicePixelRatio || 1;
    const width = Math.max(1, Math.round(canvas.clientWidth * pixel));
    const height = Math.max(1, Math.round(canvas.clientHeight * pixel));
    if (canvas.width !== width || canvas.height !== height) {
      canvas.width = width;
      canvas.height = height;
    }
    context.setTransform(1, 0, 0, 1, 0, 0);
    context.fillStyle = boardBackground;
    context.fillRect(0, 0, width, height);
    if (!frame) return;

    const { unit, x, y } = transform();
    context.setTransform(unit, 0, 0, unit, x, y);
    if (frame.scent) {
      context.imageSmoothingEnabled = false;
      context.drawImage(scent(frame.scent, size), 0, 0);
    }
    context.strokeStyle = unit < 5 ? '#657e9318' : '#657e9330';
    context.lineWidth = Math.min(.2, 1 / unit);
    context.stroke(grid);
    const progress = animation ? Math.max(0, Math.min(1, (now - animation.start) / animation.duration)) : 1;
    drawPellets(context, progress < .8 ? animation.previous.pellets : frame.pellets, size);
    drawWorms(context, frame, size, animation?.sources, progress, {
      left: -x / unit, right: (width - x) / unit, top: -y / unit, bottom: (height - y) / unit,
    });
    drawRoomSignals(context, frame.roomSignals, size, unit, roomColors.request, pixel);
    drawRoomSignals(context, frame.forwardedRoomSignals, size, unit, roomColors.forwarded, pixel);
    if (selection != null) {
      context.strokeStyle = wormColors[0];
      context.lineWidth = Math.min(.22, Math.max(.1, 1.75 * pixel / unit));
      context.strokeRect(selection % size + .08, Math.floor(selection / size) + .08, .84, .84);
    }
    if (progress < 1) redraw();
    else animation = undefined;
  }

  function redraw() {
    if (!pendingPaint) pendingPaint = requestAnimationFrame(paint);
  }

  function point(event) {
    const rect = canvas.getBoundingClientRect();
    const { unit, x, y } = transform();
    return {
      x: ((event.clientX - rect.left) * canvas.width / rect.width - x) / (unit * size),
      y: ((event.clientY - rect.top) * canvas.height / rect.height - y) / (unit * size),
    };
  }

  function pan(x, y) {
    const half = 50 / zoom;
    centerX = Math.max(half, Math.min(1 - half, x));
    centerY = Math.max(half, Math.min(1 - half, y));
    redraw();
  }

  function setZoom(value, anchor = { x: centerX, y: centerY }) {
    const next = Math.max(100, Math.min(800, Math.round(value)));
    const ratio = zoom / next;
    zoom = next;
    canvas.classList.toggle('zoomed', zoom > 100);
    pan(anchor.x + (centerX - anchor.x) * ratio, anchor.y + (centerY - anchor.y) * ratio);
  }

  new ResizeObserver(redraw).observe(canvas);
  canvas.addEventListener('wheel', event => {
    if (!size || event.ctrlKey || event.metaKey || !event.deltaY) return;
    event.preventDefault();
    setZoom(zoom * (event.deltaY < 0 ? 1.2 : 1 / 1.2), point(event));
    onZoom(zoom);
  }, { passive: false });
  canvas.addEventListener('keydown', event => {
    if (!size || !['+', '=', '-', '0'].includes(event.key)) return;
    event.preventDefault();
    setZoom(event.key === '0' ? 100 : zoom * (event.key === '-' ? 1 / 1.2 : 1.2));
    onZoom(zoom);
  });
  canvas.addEventListener('pointerdown', event => {
    drag = undefined;
    if (size && zoom > 100 && event.button === 0) {
      drag = { id: event.pointerId, x: event.clientX, y: event.clientY, centerX, centerY, moved: false };
    }
  });
  canvas.addEventListener('pointermove', event => {
    if (drag?.id !== event.pointerId) return;
    const dx = event.clientX - drag.x;
    const dy = event.clientY - drag.y;
    if (!drag.moved && Math.hypot(dx, dy) < 5) return;
    drag.moved = true;
    canvas.setPointerCapture(event.pointerId);
    const rect = canvas.getBoundingClientRect();
    pan(drag.centerX - dx * 100 / (rect.width * zoom), drag.centerY - dy * 100 / (rect.height * zoom));
  });
  canvas.addEventListener('pointercancel', () => { drag = undefined; });
  canvas.addEventListener('click', event => {
    if (!size || drag?.moved) return;
    const p = point(event);
    if (p.x >= 0 && p.x < 1 && p.y >= 0 && p.y < 1) {
      onSelect(Math.floor(p.y * size) * size + Math.floor(p.x * size));
    }
  });

  return {
    reset(nextSize, percent) {
      size = nextSize;
      grid = gridPath(size);
      frame = undefined;
      animation = undefined;
      centerX = centerY = .5;
      selection = null;
      setZoom(percent);
    },
    setZoom,
    select(index) {
      selection = index;
      redraw();
    },
    show(next, duration = 0) {
      animation = duration && frame && !reducedMotion.matches ? {
        start: performance.now(), duration, previous: frame, sources: wormStepSources(next.bodies, frame.bodies),
      } : undefined;
      frame = next;
      redraw();
    },
  };
}
