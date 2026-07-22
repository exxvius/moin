export function HomeView() {
  return (
    <div className="view">
      <div className="view-head">
        <h2>Add a download</h2>
        <p>
          Paste a link or magnet, or drop a .torrent file. moin handles direct
          downloads, torrents, and media sites — all in one queue.
        </p>
      </div>

      <div className="card">
        <div className="card-title">Coming together</div>
        <p className="dim">
          The add flow lands in Phase 2 (direct HTTP) and Phase 3 (BitTorrent).
        </p>
      </div>
    </div>
  );
}
