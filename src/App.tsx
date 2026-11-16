import { Activity, ArrowUpRight, Cable, CircleHelp, Database, Globe2, HardDrive, LayoutDashboard, ListTree, Plus, Server, Settings, ShieldCheck, TerminalSquare } from "lucide-react";

const nav = [
  { label: "Overview", icon: LayoutDashboard },
  { label: "Websites", icon: Globe2, count: "0" },
  { label: "Domains", icon: Cable },
  { label: "Databases", icon: Database },
  { label: "Deployments", icon: ArrowUpRight },
  { label: "File Manager", icon: HardDrive },
  { label: "Networking", icon: Server },
  { label: "SSL & Security", icon: ShieldCheck },
  { label: "Monitoring", icon: Activity },
  { label: "Logs", icon: TerminalSquare },
  { label: "Backups", icon: ListTree },
];

function App() {
  return <div className="shell">
    <aside className="sidebar">
      <div className="brand"><div className="brand-mark">W</div><div><strong>WebDynamite</strong><small>HOSTING CONTROL CENTER</small></div></div>
      <div className="server-card"><span className="pulse" /> <div><b>Local server</b><small>Core service not connected</small></div><span className="status">OFFLINE</span></div>
      <div className="nav-label">WORKSPACE</div>
      <nav>{nav.map(({label,icon:Icon,count})=><button className={label==="Overview"?"nav-item active":"nav-item"} key={label}><Icon size={17}/><span>{label}</span>{count&&<small>{count}</small>}</button>)}</nav>
      <div className="sidebar-bottom"><button className="nav-item"><Settings size={17}/>Settings</button><button className="nav-item"><CircleHelp size={17}/>Help & diagnostics</button><div className="version">WEBDYNAMITE <span>v0.1.0</span></div></div>
    </aside>
    <main className="main">
      <header className="topbar"><div className="crumb">Workspace <span>/</span> <b>Overview</b></div><div className="top-actions"><span className="connection"><i/> Core service disconnected</span><button className="icon-button"><CircleHelp size={18}/></button><div className="avatar">MB</div></div></header>
      <section className="content">
        <div className="page-heading"><div><div className="eyebrow">YOUR PC. YOUR SERVER. YOUR WEBSITES.</div><h1>Overview</h1><p>Everything happening across your hosting environment.</p></div><button className="primary" disabled><Plus size={17}/> Add website <span className="soon">SERVICE REQUIRED</span></button></div>
        <div className="notice"><div className="notice-icon"><Server size={19}/></div><div><b>Core Service is not connected</b><p>The desktop interface is ready, but hosting controls will activate when the WebDynamite Core Service is installed and running.</p></div><span className="notice-tag">FOUNDATION BUILD</span></div>
        <div className="stats">{[["WEBSITES","0","Registered projects"],["RUNNING","0","Active processes"],["DOMAINS","0","Configured routes"],["UPTIME","—","Service unavailable"]].map(([k,v,d])=><div className="stat" key={k}><span>{k}</span><strong>{v}</strong><small>{d}</small></div>)}</div>
        <div className="grid"><div className="panel"><div className="panel-head"><div><h2>Websites</h2><p>Your managed projects will appear here.</p></div><button className="subtle" disabled>View all <ArrowUpRight size={14}/></button></div><div className="empty"><div className="empty-icon"><Globe2 size={25}/></div><b>No websites yet</b><p>Add a project folder to start managing your websites from one place.</p><button className="outline" disabled><Plus size={15}/> Add your first website</button></div></div>
        <div className="panel"><div className="panel-head"><div><h2>System health</h2><p>Host machine overview</p></div><Activity size={17} className="muted"/></div><div className="health-row"><span>CPU usage</span><b>—</b></div><div className="health-row"><span>Memory usage</span><b>—</b></div><div className="health-row"><span>Disk usage</span><b>—</b></div><div className="health-row"><span>Network activity</span><b>—</b></div><div className="health-foot">Metrics will be supplied by the Core Service.</div></div></div>
        <footer>WebDynamite <span>·</span> Built for developers who host their own.</footer>
      </section>
    </main>
  </div>;
}
export default App;
