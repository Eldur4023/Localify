// test_boot.cpp — arranca el LuxServer de src/boot.cpp en un directorio dado
// (para probar el arranque móvil sin un teléfono).
#include "boot.hpp"

#include <iostream>

int main(int argc, char** argv) {
    if (argc < 2) { std::cerr << "uso: test_boot <work_dir> [puerto]\n"; return 2; }
    try {
        LuxServer server(argv[1]);
        uint16_t port = server.start();
        std::cout << "puerto: " << port << "\n" << std::flush;
        // servir 60 s y salir
        std::this_thread::sleep_for(std::chrono::seconds(60));
    } catch (const std::exception& e) {
        std::cerr << "error: " << e.what() << "\n";
        return 1;
    }
    return 0;
}
