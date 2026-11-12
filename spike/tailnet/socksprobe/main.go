// Spike only: a SOCKS5 proxy that logs every request it is asked to make,
// with the same username/password shape as tsnet's Loopback() proxy.
// It answers "what does WKWebView send through proxyConfigurations, and in
// what form (hostname or a locally resolved IP)?" without a tailnet.
// Names under -map suffixes go to the mapped address; the rest are refused
// unless -pass is set, so nothing silently works around the test.
package main

import (
	"encoding/binary"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"log"
	"net"
	"os"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

var (
	listen = flag.String("listen", "127.0.0.1:11080", "listen address")
	user   = flag.String("user", "tsnet", "required username")
	pass   = flag.String("pass", "", "required password")
	maps   = flag.String("map", "", "comma list of suffix=host:port")
	passOK = flag.Bool("pass-other", false, "dial names not under -map directly")
	logf   = flag.String("log", "socks.jsonl", "JSONL log")
)

var (
	mu  sync.Mutex
	out *os.File
	seq atomic.Int64
)

func emit(v map[string]any) {
	v["t"] = time.Now().UTC().Format(time.RFC3339Nano)
	b, _ := json.Marshal(v)
	mu.Lock()
	out.Write(append(b, '\n'))
	mu.Unlock()
}

func main() {
	flag.Parse()
	var err error
	out, err = os.OpenFile(*logf, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o644)
	if err != nil {
		log.Fatal(err)
	}
	m := map[string]string{}
	for _, kv := range strings.Split(*maps, ",") {
		if k, v, ok := strings.Cut(kv, "="); ok {
			m[strings.ToLower(k)] = v
		}
	}
	ln, err := net.Listen("tcp", *listen)
	if err != nil {
		log.Fatal(err)
	}
	log.Printf("socksprobe on %s map=%v pass-other=%v", *listen, m, *passOK)
	for {
		c, err := ln.Accept()
		if err != nil {
			log.Fatal(err)
		}
		go handle(c, m)
	}
}

func handle(c net.Conn, m map[string]string) {
	defer c.Close()
	id := seq.Add(1)
	c.SetDeadline(time.Now().Add(10 * time.Second))
	var b [262]byte
	if _, err := io.ReadFull(c, b[:2]); err != nil || b[0] != 5 {
		emit(map[string]any{"id": id, "err": "not socks5"})
		return
	}
	methods := make([]byte, b[1])
	io.ReadFull(c, methods)
	hasUP := false
	for _, x := range methods {
		hasUP = hasUP || x == 2
	}
	if !hasUP {
		c.Write([]byte{5, 0xff})
		emit(map[string]any{"id": id, "err": "client offered no user/pass", "methods": methods})
		return
	}
	c.Write([]byte{5, 2})
	io.ReadFull(c, b[:2]) // ver, ulen
	u := make([]byte, b[1])
	io.ReadFull(c, u)
	io.ReadFull(c, b[:1])
	p := make([]byte, b[0])
	io.ReadFull(c, p)
	if string(u) != *user || string(p) != *pass {
		c.Write([]byte{1, 1})
		emit(map[string]any{"id": id, "err": "bad credentials"})
		return
	}
	c.Write([]byte{1, 0})
	io.ReadFull(c, b[:4]) // ver cmd rsv atyp
	cmd, atyp := b[1], b[3]
	var host, kind string
	switch atyp {
	case 1:
		io.ReadFull(c, b[:4])
		host, kind = net.IP(b[:4]).String(), "ipv4"
	case 3:
		io.ReadFull(c, b[:1])
		n := int(b[0])
		io.ReadFull(c, b[:n])
		host, kind = string(b[:n]), "domain"
	case 4:
		io.ReadFull(c, b[:16])
		host, kind = net.IP(b[:16]).String(), "ipv6"
	}
	io.ReadFull(c, b[:2])
	port := binary.BigEndian.Uint16(b[:2])
	ev := map[string]any{"id": id, "cmd": cmd, "atyp": kind, "host": host, "port": port}
	if cmd != 1 {
		c.Write([]byte{5, 7, 0, 1, 0, 0, 0, 0, 0, 0})
		ev["err"] = "only CONNECT"
		emit(ev)
		return
	}
	target := ""
	lh := strings.ToLower(host)
	for suf, addr := range m {
		if lh == suf || strings.HasSuffix(lh, "."+suf) {
			target = addr
		}
	}
	if target == "" && *passOK {
		target = net.JoinHostPort(host, fmt.Sprint(port))
	}
	if target == "" {
		c.Write([]byte{5, 2, 0, 1, 0, 0, 0, 0, 0, 0}) // not allowed by ruleset
		ev["err"] = "refused (not mapped)"
		emit(ev)
		return
	}
	t0 := time.Now()
	up, err := net.DialTimeout("tcp", target, 5*time.Second)
	if err != nil {
		c.Write([]byte{5, 5, 0, 1, 0, 0, 0, 0, 0, 0})
		ev["err"] = err.Error()
		emit(ev)
		return
	}
	defer up.Close()
	c.Write([]byte{5, 0, 0, 1, 127, 0, 0, 1, 0, 0})
	c.SetDeadline(time.Time{})
	ev["via"] = target
	emit(ev)
	var in, outb int64
	done := make(chan struct{}, 2)
	go func() { outb, _ = io.Copy(up, c); up.(*net.TCPConn).CloseWrite(); done <- struct{}{} }()
	go func() { in, _ = io.Copy(c, up); done <- struct{}{} }()
	<-done
	<-done
	emit(map[string]any{"id": id, "closed": true, "host": host, "up": outb, "down": in, "secs": time.Since(t0).Seconds()})
}
