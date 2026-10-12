const W = 10, H = 20;
const SHAPES = [
  [[0, 0], [1, 0], [0, 1], [1, 1]], [[0, 0], [1, 0], [2, 0], [3, 0]],
  [[0, 0], [1, 0], [2, 0], [1, 1]], [[0, 0], [0, 1], [1, 1], [2, 1]],
  [[2, 0], [0, 1], [1, 1], [2, 1]], [[1, 0], [2, 0], [0, 1], [1, 1]],
  [[0, 0], [1, 0], [1, 1], [2, 1]],
];
const COLORS = ['#f4d35e', '#4cc9f0', '#b388ff', '#80ed99', '#ff9f1c', '#ff5d8f', '#72efdd'];

// Direct port of apps/twinkle-blocks/src/main.rs::Game.
export class Game {
  constructor(source = null) {
    Object.assign(this, source ?? {
      cells: Array(W * H).fill(null), shape: 1, x: 3, y: 0, rotation: 0,
      score: 0, lines: 0, next: 2, paused: false, over: false,
    });
  }
  static shapeCoords(shape, rotation) {
    if (shape === 0) return SHAPES[0];
    if (shape === 1) return [
      [[0, 1], [1, 1], [2, 1], [3, 1]], [[2, 0], [2, 1], [2, 2], [2, 3]],
      [[0, 2], [1, 2], [2, 2], [3, 2]], [[1, 0], [1, 1], [1, 2], [1, 3]],
    ][rotation % 4];
    return SHAPES[shape].map(([x, y]) => [[x, y], [2 - y, x], [2 - x, 2 - y], [y, 2 - x]][rotation % 4]);
  }
  coords() { return Game.shapeCoords(this.shape, this.rotation); }
  fits(x, y, rotation) {
    return Game.shapeCoords(this.shape, rotation).every(([px, py]) => {
      const cx = x + px, cy = y + py;
      return cx >= 0 && cx < W && cy < H && (cy < 0 || this.cells[cy * W + cx] == null);
    });
  }
  lockPiece() {
    for (const [px, py] of this.coords()) {
      const x = this.x + px, y = this.y + py;
      if (y >= 0 && y < H && x >= 0 && x < W) this.cells[y * W + x] = this.shape;
    }
    const kept = [];
    let cleared = 0;
    for (let row = 0; row < H; row++) {
      const cells = this.cells.slice(row * W, (row + 1) * W);
      if (cells.every(cell => cell != null)) cleared++;
      else kept.push(...cells);
    }
    for (let row = 0; row < cleared; row++) kept.unshift(...Array(W).fill(null));
    this.cells = kept;
    this.lines += cleared;
    this.score += [0, 100, 300, 500, 800][Math.min(cleared, 4)] * this.level();
    this.shape = this.next;
    this.next = (this.next * 5 + this.lines + 3) % 7;
    this.x = 3; this.y = 0; this.rotation = 0;
    this.over = !this.fits(this.x, this.y, this.rotation);
  }
  level() { return 1 + Math.floor(this.lines / 10); }
  gravityInterval() { return Math.max(90, 700 - (this.level() - 1) * 55); }
  softDropInterval() { return Math.max(25, Math.floor(this.gravityInterval() / 12)); }
  gravityTick() {
    if (this.fits(this.x, this.y + 1, this.rotation)) this.y++;
    else this.lockPiece();
  }
  act(action) {
    if (action === 'restart') { Object.assign(this, new Game()); return; }
    if (action === 'toggle') { if (!this.over) this.paused = !this.paused; return; }
    if (this.paused || this.over) return;
    switch (action) {
      case 'left': if (this.fits(this.x - 1, this.y, this.rotation)) this.x--; break;
      case 'right': if (this.fits(this.x + 1, this.y, this.rotation)) this.x++; break;
      case 'rotate': {
        if (this.shape === 0) return;
        const next = (this.rotation + 1) & 0xff;
        const kick = [0, -1, 1, -2, 2].find(dx => this.fits(this.x + dx, this.y, next));
        if (kick !== undefined) { this.x += kick; this.rotation = next; }
        break;
      }
      case 'down':
        if (this.fits(this.x, this.y + 1, this.rotation)) { this.y++; this.score++; }
        else this.lockPiece();
        break;
      case 'drop': {
        let distance = 0;
        while (this.fits(this.x, this.y + 1, this.rotation)) { this.y++; distance++; }
        this.score += distance * 2; this.lockPiece(); break;
      }
    }
  }
  snapshot(menuOpen) {
    const board = [...this.cells];
    for (const [px, py] of this.coords()) {
      const x = this.x + px, y = this.y + py;
      if (y >= 0 && y < H && x >= 0 && x < W) board[y * W + x] = this.shape;
    }
    return {
      board: board.map(cell => cell == null ? '' : `c${cell}`), next: COLORS[this.next],
      score: this.score, lines: this.lines, level: this.level(), paused: this.paused,
      over: this.over, menuOpen,
      message: this.over ? 'Stack topped out' : this.paused ? 'Ready — choose Start' : 'In play',
    };
  }
}
