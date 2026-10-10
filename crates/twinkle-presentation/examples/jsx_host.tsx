import './jsx_host.css';

export default function App() {
    const [count, setCount] = useState(0);
    return <Window id="main" width={640} height={360} title="Twinkle JSX host">
        <Column className="app">
            <Text>Rust host and native JSX</Text>
            <Text>Rust validates each requested operation before accepting the next UI tree.</Text>
            <Button className="action" id="increment" onClick={() => {
                const next = count + 1;
                setCount(next);
                twinkle.request({type: 'set-count', value: next});
            }}>
                {'Count: ' + count}
            </Button>
        </Column>
    </Window>;
}
