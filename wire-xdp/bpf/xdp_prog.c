#include <linux/bpf.h>
#include <linux/in.h>
#include <linux/if_ether.h>
#include <linux/ip.h>
#include <linux/tcp.h>
#include <linux/udp.h>
#include <bpf/bpf_helpers.h>

struct {
    __uint(type, BPF_MAP_TYPE_XSKMAP);
    __uint(max_entries, 64);
    __uint(key_size, sizeof(int));
    __uint(value_size, sizeof(int));
} xsks_map SEC(".maps");

struct {
    __uint(type, BPF_MAP_TYPE_HASH);
    __uint(max_entries, 256);
    __uint(key_size, sizeof(__u16));
    __uint(value_size, sizeof(__u8));
} allowed_ports SEC(".maps");

SEC("xdp")
int xdp_redirect_xsk(struct xdp_md *ctx) {
    void *data_end = (void *)(long)ctx->data_end;
    void *data = (void *)(long)ctx->data;

    struct ethhdr *eth = data;
    if ((void *)(eth + 1) > data_end) {
        return XDP_PASS;
    }

    if (eth->h_proto != __constant_htons(ETH_P_IP)) {
        return XDP_PASS;
    }

    struct iphdr *iph = (void *)(eth + 1);
    if ((void *)(iph + 1) > data_end) {
        return XDP_PASS;
    }

    __u16 dport = 0;
    if (iph->protocol == IPPROTO_TCP) {
        struct tcphdr *th = (void *)((__u32 *)iph + iph->ihl);
        if ((void *)(th + 1) > data_end) {
            return XDP_PASS;
        }
        dport = th->dest;
    } else if (iph->protocol == IPPROTO_UDP) {
        struct udphdr *uh = (void *)((__u32 *)iph + iph->ihl);
        if ((void *)(uh + 1) > data_end) {
            return XDP_PASS;
        }
        dport = uh->dest;
    } else {
        return XDP_PASS;
    }

    __u8 *val = bpf_map_lookup_elem(&allowed_ports, &dport);
    if (!val) {
        return XDP_PASS;
    }

    int index = ctx->rx_queue_index;
    if (bpf_map_lookup_elem(&xsks_map, &index)) {
        return bpf_redirect_map(&xsks_map, index, 0);
    }

    return XDP_PASS;
}

char _license[] SEC("license") = "GPL";
