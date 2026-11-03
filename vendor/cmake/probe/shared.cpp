#if defined(TERRA_EXPECT_MOCHIOS) && (!defined(__mochios__) || defined(__linux__))
#error Expected mochiOS target headers/compiler macros, not a host Linux compile
#endif
#include <atomic>
#include <stdexcept>
#include <thread>
#include <vector>
static_assert(__cplusplus > 202002L);
extern "C" int terra_webkit_toolchain_probe() {
    std::atomic<int> value{0};
    std::thread worker([&] { value.store(42); });
    worker.join();
    std::vector<int> values{value.load()};
    try {
        throw std::runtime_error("unwind probe");
    } catch (const std::exception&) {
        return values.front();
    }
}
