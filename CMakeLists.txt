cmake_minimum_required(VERSION 3.20)
project(potato_gsim LANGUAGES CXX)

set(CMAKE_CXX_STANDARD 23)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

if(NOT CMAKE_CONFIGURATION_TYPES AND NOT CMAKE_BUILD_TYPE)
  set(CMAKE_BUILD_TYPE Release CACHE STRING "Build type" FORCE)
endif()

find_package(SFML 3 REQUIRED COMPONENTS Graphics Window System)

add_executable(
  potato_gsim
  main.cpp
  src/io/NativeFileDialog.cpp
  src/io/Persistence.cpp
  src/render/Renderer.cpp
  src/scenario/ScenarioManager.cpp
  src/sim/QuadTree.cpp
  src/sim/SimulationEngine.cpp
  src/sim/ThreadPool.cpp
  src/sim/UniformGrid.cpp
  src/ui/CreationTool.cpp
  src/ui/CreationPauseStateMachine.cpp
  src/ui/InputActions.cpp
  src/ui/InspectorOverlay.cpp
  src/ui/MenuOverlay.cpp
  src/ui/Predictor.cpp
  src/ui/QuantityFormat.cpp
  src/ui/SelectionState.cpp
  src/ui/TraceStore.cpp
)
target_include_directories(potato_gsim PRIVATE src)
target_link_libraries(potato_gsim PRIVATE SFML::Graphics SFML::Window SFML::System)

if(MSVC)
  target_compile_options(potato_gsim PRIVATE /W4 /permissive-)
else()
  target_compile_options(potato_gsim PRIVATE -Wall -Wextra -Wpedantic)
endif()

include(GNUInstallDirs)
install(TARGETS potato_gsim
  RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR}
  BUNDLE DESTINATION .
)

include(CTest)
if(BUILD_TESTING)
  add_executable(
    parity_regression_tests
    tests/parity_regression_tests.cpp
    src/io/Persistence.cpp
    src/scenario/ScenarioManager.cpp
    src/ui/CreationTool.cpp
    src/ui/QuantityFormat.cpp
  )
  target_include_directories(parity_regression_tests PRIVATE src)
  target_link_libraries(parity_regression_tests PRIVATE SFML::System)

  add_executable(
    high_load_smoke
    tests/high_load_smoke.cpp
    src/scenario/ScenarioManager.cpp
    src/sim/QuadTree.cpp
    src/sim/SimulationEngine.cpp
    src/sim/ThreadPool.cpp
    src/sim/UniformGrid.cpp
  )
  target_include_directories(high_load_smoke PRIVATE src)
  target_link_libraries(high_load_smoke PRIVATE SFML::System)

  add_executable(
    throughput_benchmark
    tests/throughput_benchmark.cpp
    src/scenario/ScenarioManager.cpp
    src/sim/QuadTree.cpp
    src/sim/SimulationEngine.cpp
    src/sim/ThreadPool.cpp
    src/sim/UniformGrid.cpp
  )
  target_include_directories(throughput_benchmark PRIVATE src)
  target_link_libraries(throughput_benchmark PRIVATE SFML::System)

  add_executable(
    algorithm_switching_tests
    tests/algorithm_switching_tests.cpp
    src/sim/QuadTree.cpp
    src/sim/SimulationEngine.cpp
    src/sim/ThreadPool.cpp
    src/sim/UniformGrid.cpp
  )
  target_include_directories(algorithm_switching_tests PRIVATE src)
  target_link_libraries(algorithm_switching_tests PRIVATE SFML::System)

  add_test(NAME parity_regression_tests COMMAND parity_regression_tests)
  add_test(NAME high_load_smoke COMMAND high_load_smoke)
  add_test(NAME algorithm_switching_tests COMMAND algorithm_switching_tests)
endif()
