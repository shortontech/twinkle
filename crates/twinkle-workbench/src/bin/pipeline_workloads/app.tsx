/// <reference path="../../../twinkle/crates/twinkle-jsx-runtime/types/twinkle.d.ts" />
import './app.css';

const act = (action: string) => twinkle.request({action});
const Cell = memo(function Cell({color}: {color: string}) {
    return <Div className={color ? 'cell filled ' + color : 'cell'} />;
});

export default function App() {
    const board = (twinkle.data.board as string[]) || [];
    if (twinkle.data.menuOpen || twinkle.data.over) {
        return <Window id="main" width={800} height={720} title="Twinkle Blocks" background={0xff101820}>
            <Column className="menu-screen">
                <Text className="menu-title">{twinkle.data.over ? 'Game over' : 'Twinkle Blocks'}</Text>
                <Text className="menu-score">{'Score ' + twinkle.data.score + '  ·  Lines ' + twinkle.data.lines + '  ·  Level ' + twinkle.data.level}</Text>
                <Column className="menu-card">
                    {!twinkle.data.over && <Button className="menu-action primary" id="continue" onClick={() => act('menu')}>Continue</Button>}
                    <Button className="menu-action" id="restart" onClick={() => act('restart')}>New game</Button>
                    <Text className="menu-help">D-pad or left stick: move and soft drop</Text>
                    <Text className="menu-help">A: rotate · B: hard drop · Start: resume</Text>
                </Column>
            </Column>
        </Window>;
    }
    return <Window id="main" width={800} height={720} title="Twinkle Blocks" background={0xff101820}>
        <Column className="app">
            <Row className="topbar">
                <Text className="brand">Twinkle Blocks</Text>
                <Text>{'Score ' + twinkle.data.score + '  ·  Lines ' + twinkle.data.lines + '  ·  Level ' + twinkle.data.level}</Text>
                <Text className="menu-hint">Start / Esc: game menu</Text>
            </Row>
            <Row className="play">
                <div className="board" role="group" aria-label="Game board">{board.map((color, index) => <Cell key={index} color={color} />)}</div>
                <Column className="side">
                    <Text className={twinkle.data.over ? 'status game-over' : 'status'}>{String(twinkle.data.message || '')}</Text>
                    <Text>Next</Text>
                    <Div className="next-piece"><Div className="next-block" /></Div>
                    <Text>Controls</Text>
                    <Text className="hint">D-pad / left stick: move and drop · A: rotate · B: hard drop · Start: game menu. Keyboard: ↑/↓, Alt+←/→, Enter, Escape.</Text>
                </Column>
            </Row>
        </Column>
    </Window>;
}
